import 'dart:async';
import 'dart:io';
import 'dart:typed_data';

import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:flutter/material.dart' hide Overlay;
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:permission_handler/permission_handler.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:shared_preferences/util/legacy_to_async_migration_util.dart';
import 'package:telepathy/app.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/core/utils/index.dart';
import 'package:telepathy/src/rust/audio/player.dart';
import 'package:telepathy/src/rust/flutter.dart';
import 'package:telepathy/src/rust/frb_generated.dart';
import 'package:telepathy/src/rust/overlay/overlay.dart';
import 'package:telepathy/src/rust/telepathy.dart';
import 'package:window_manager/window_manager.dart';

Future<void> main(List<String> args) async {
  WidgetsFlutterBinding.ensureInitialized();
  if (!kIsWeb) {
    await windowManager.ensureInitialized();
  }

  try {
    await RustLib.init();
  } catch (e, st) {
    debugPrint('RustLib.init failed: $e');
    debugPrint('$st');
    rethrow;
  }

  // get logs from rust
  rustSetUp();
  createLogStream().listen((message) {
    DebugConsole.log(message);
  });

  if (kIsWeb) {
    PermissionStatus status = await Permission.microphone.request();

    if (!status.isGranted) {
      DebugConsole.error('Microphone permission not accepted');
    }
  } else {
    if (Platform.isAndroid || Platform.isIOS) {
      PermissionStatus status = await Permission.microphone.request();

      if (!status.isGranted) {
        DebugConsole.error('Microphone permission not accepted');
      }
    }
  }

  const storage = FlutterSecureStorage();

  final legacy = await SharedPreferences.getInstance();

  await migrateLegacySharedPreferencesToSharedPreferencesAsyncIfNecessary(
    legacySharedPreferencesInstance: legacy,
    sharedPreferencesAsyncOptions: const SharedPreferencesOptions(),
    migrationCompletedKey: 'prefs_migrated_to_async_v1',
  );
  final SharedPreferencesAsync options = SharedPreferencesAsync();

  final ProfilesController profilesController =
      ProfilesController(storage: storage, options: options);
  await profilesController.init(args);

  final AudioSettingsController audioSettingsController =
      AudioSettingsController(options: options);
  await audioSettingsController.init();

  final NetworkSettingsController networkSettingsController =
      NetworkSettingsController(options: options);
  await networkSettingsController.init();

  final PreferencesController preferencesController =
      PreferencesController(options: options);
  await preferencesController.init();

  final StateController stateController = StateController();
  final StatisticsController statisticsController = StatisticsController();

  final Overlay overlay = await Overlay.newInstance(
    enabled: networkSettingsController.overlayConfig.enabled,
    x: networkSettingsController.overlayConfig.x.round(),
    y: networkSettingsController.overlayConfig.y.round(),
    width: networkSettingsController.overlayConfig.width.round(),
    height: networkSettingsController.overlayConfig.height.round(),
    fontHeight: networkSettingsController.overlayConfig.fontHeight,
    backgroundColor:
        networkSettingsController.overlayConfig.backgroundColor.toARGB32(),
    fontColor: networkSettingsController.overlayConfig.fontColor.toARGB32(),
  );

  final soundPlayer =
      SoundPlayer(outputVolume: audioSettingsController.soundVolume);
  soundPlayer.updateOutputDevice(name: audioSettingsController.outputDevice);
  soundPlayer.updateOutputVolume(volume: audioSettingsController.soundVolume);

  ArcHost host = soundPlayer.host();

  final chatStateController = ChatStateController(soundPlayer);

  /// called when there is an incoming call
  FutureOr<bool> acceptCall(
      (String id, Uint8List? ringtone, DartNotify cancel) record) async {
    final (String id, Uint8List? ringtone, DartNotify cancel) = record;

    Contact? contact = profilesController.getContact(id);

    if (stateController.isCallActive) {
      return false;
    } else if (contact == null) {
      DebugConsole.warn('contact is null');
      return false;
    }

    List<int> bytes;

    if (ringtone == null) {
      bytes = await readSeaBytes('incoming');
    } else {
      bytes = ringtone;
    }

    SoundHandle handle = await soundPlayer.play(bytes: bytes);

    if (navigatorKey.currentState == null ||
        !navigatorKey.currentState!.mounted) {
      handle.cancel();
      return false;
    }

    Future acceptedFuture =
        acceptCallPrompt(navigatorKey.currentState!.context, contact);
    Future cancelFuture = cancel.notified();

    final result = await Future.any([acceptedFuture, cancelFuture]);

    handle.cancel();

    if (result == null) {
      DebugConsole.debug('cancelled');

      if (navigatorKey.currentState != null &&
          navigatorKey.currentState!.mounted) {
        Navigator.pop(navigatorKey.currentState!.context);
      }

      return false; // cancelled
    } else if (result) {
      stateController.setStatus('Connecting');
      stateController.setActiveContact(contact);
    }

    return result;
  }

  /// called when a contact is needed in the backend
  Contact? getContact(Uint8List peerId) {
    try {
      Contact? contact = profilesController.contacts.values
          .firstWhere((Contact contact) => contact.idEq(id: peerId));
      return contact.pubClone();
    } catch (_) {
      return null;
    }
  }

  /// called when the call state changes
  FutureOr<void> callState(CallState state) async {
    if (!stateController.isCallActive) {
      return;
    }

    // ensure the outgoing sound has been canceled as the call is now active
    outgoingSoundHandle?.cancel();
    List<int> bytes;

    switch (state) {
      case CallState_Connected():
        // handles the initial connect
        bytes = await readSeaBytes('connected');
        stateController.setStatus('Active');
      case CallState_Waiting():
        stateController.setStatus('Waiting for peers');
        return;
      case CallState_RoomJoin():
        stateController.roomJoin(state.field0);
        return; // TODO add room join sound
      case CallState_RoomLeave():
        stateController.roomLeave(state.field0);
        return; // TODO add room leave sound
      case CallState_CallEnded():
        if (!stateController.isCallActive) {
          DebugConsole.warn('call ended entered but there is no active call');
          return;
        }

        stateController.endOfCall();
        bytes = await readSeaBytes('call_ended');

        if (state.field0.isNotEmpty &&
            navigatorKey.currentState != null &&
            navigatorKey.currentState!.mounted) {
          showErrorDialog(
              navigatorKey.currentState!.context,
              state.field1 ? 'Call failed (remote)' : 'Call failed',
              state.field0);
        }
    }

    otherSoundHandle = await soundPlayer.play(bytes: bytes);
  }

  /// called when the backend wants to start sessions
  List<Contact> getContacts(_) {
    return profilesController.contacts.values.map((c) => c.pubClone()).toList();
  }

  FlutterCallbacks callbacks = FlutterCallbacks(
      acceptCall: acceptCall,
      getContact: getContact,
      callState: callState,
      sessionStatus: stateController.updateSession,
      getContacts: getContacts,
      statistics: statisticsController.setStatistics,
      messageReceived: chatStateController.messageReceived,
      managerActive: stateController.setSessionManager,
      screenshareStarted: stateController.screenshareStarted);

  final telepathy = Telepathy(
      host: host,
      networkConfig: networkSettingsController.networkConfig,
      screenshareConfig: networkSettingsController.screenshareConfig,
      overlay: overlay,
      codecConfig: networkSettingsController.codecConfig,
      callbacks: callbacks);

  await telepathy.setIdentity(key: profilesController.keypair);
  await telepathy.startManager();

  // attempt to open sessions with all contacts
  for (Contact contact in profilesController.contacts.values) {
    telepathy.startSession(contact: contact);
  }

  final audioDevices = AudioDevices(telepathy: telepathy);

  // apply options to the instance
  telepathy.setRmsThreshold(decimal: audioSettingsController.inputSensitivity);
  telepathy.setInputVolume(decibel: audioSettingsController.inputVolume);
  telepathy.setOutputVolume(decibel: audioSettingsController.outputVolume);
  telepathy.setDenoise(denoise: audioSettingsController.useDenoise);
  telepathy.setPlayCustomRingtones(
      play: preferencesController.playCustomRingtones);
  telepathy.setInputDevice(device: audioSettingsController.inputDevice);
  telepathy.setOutputDevice(device: audioSettingsController.outputDevice);
  telepathy.setSendCustomRingtone(
      send: preferencesController.customRingtoneFile != null);
  telepathy.setEfficiencyMode(enabled: preferencesController.efficiencyMode);

  if (audioSettingsController.denoiseModel != null) {
    updateDenoiseModel(audioSettingsController.denoiseModel, telepathy);
  }

  final InterfaceController interfaceController =
      InterfaceController(options: options);
  await interfaceController.init();

  runApp(TelepathyApp(
    telepathy: telepathy,
    profilesController: profilesController,
    audioSettingsController: audioSettingsController,
    networkSettingsController: networkSettingsController,
    preferencesController: preferencesController,
    interfaceController: interfaceController,
    callStateController: stateController,
    player: soundPlayer,
    chatStateController: chatStateController,
    statisticsController: statisticsController,
    overlay: overlay,
    audioDevices: audioDevices,
  ));
}
