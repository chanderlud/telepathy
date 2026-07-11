import 'dart:async';
import 'dart:typed_data';

import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:flutter/material.dart' hide Overlay;
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:permission_handler/permission_handler.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:shared_preferences/util/legacy_to_async_migration_util.dart';
import 'package:provider/provider.dart';
import 'package:telepathy/app.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/core/utils/index.dart';
import 'package:telepathy/core/rust/player.dart';
import 'package:telepathy/core/rust/frb_generated.dart';
import 'package:telepathy/core/rust/overlay.dart';
import 'package:telepathy/core/rust/flutter.dart';
import 'package:telepathy/core/rust/types.dart';
import 'package:window_manager/window_manager.dart';
import 'package:telepathy/core/rust/flutter/logging.dart';

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
  soundPlayer.updateOutputDevice(
      deviceId: audioSettingsController.outputDeviceId);
  soundPlayer.updateOutputVolume(volume: audioSettingsController.soundVolume);

  ArcHost host = soundPlayer.host();

  final chatStateController = ChatStateController(soundPlayer);

  /// called when there is an incoming call
  FutureOr<bool> acceptCall(
      (String id, Uint8List? ringtone, FrontendNotify cancel) record) async {
    final (String id, Uint8List? ringtone, FrontendNotify cancel) = record;

    Contact? contact = profilesController.getContact(id);

    if (stateController.hasLiveCall) {
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

    FlutterSoundHandle? handle = await playSoundEffect(
      player: soundPlayer,
      bytes: bytes,
      sound: 'incoming',
    );

    if (navigatorKey.currentState == null ||
        !navigatorKey.currentState!.mounted) {
      handle?.cancel();
      return false;
    }

    Future acceptedFuture =
        acceptCallPrompt(navigatorKey.currentState!.context, contact);
    Future cancelFuture = cancel.notified();

    final result = await Future.any([acceptedFuture, cancelFuture]);

    handle?.cancel();

    if (result == null) {
      DebugConsole.debug('cancelled');

      if (navigatorKey.currentState != null &&
          navigatorKey.currentState!.mounted) {
        Navigator.pop(navigatorKey.currentState!.context);
      }

      return false; // cancelled
    } else if (result) {
      // Move through the same connecting->active gate the outgoing
      // path uses.
      stateController.setStatus('Connecting');
      stateController.setPendingContact(contact);
      stateController.promotePendingContact(contact);
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
    if (!stateController.hasLiveCall) {
      return;
    }

    // ensure the outgoing sound has been canceled as the call is now active
    outgoingSoundHandle?.cancel();
    List<int> bytes;

    switch (state) {
      case CallState_Connected():
        // handles the initial connect
        bytes = await readSeaBytes('connected');
        // backend confirmed the call is now active; the pending target (if any)
        // has been promoted to active by the widget that initiated the call.
        stateController.clearPending();
        stateController.markCallActive();
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
        // Suppress dialogs when the local user just hung up (the widget sets
        // status to Inactive and clears state synchronously before the backend
        // `CallEnded` echo arrives). Otherwise surface the failure reason.
        final localHangup = !stateController.hasLiveCall;
        stateController.endOfCall();
        bytes = await readSeaBytes('call_ended');

        if (!localHangup &&
            state.field0.isNotEmpty &&
            navigatorKey.currentState != null &&
            navigatorKey.currentState!.mounted) {
          showErrorDialog(
              navigatorKey.currentState!.context,
              state.field1 ? 'Call failed (remote)' : 'Call failed',
              state.field0);
        }
    }

    otherSoundHandle = await playSoundEffect(
      player: soundPlayer,
      bytes: bytes,
      sound: state is CallState_Connected ? 'connected' : 'call-ended',
    );
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

  // Snapshot the output device ID before reconciliation so we can detect
  // whether pruning cleared or changed it (e.g., a saved USB headset was
  // unplugged). The sound player was already bound to the pre-prune ID at
  // construction, so without a refresh it would keep using a stale binding.
  final prePruneOutputDeviceId = audioSettingsController.outputDeviceId;

  // Reconcile persisted device selections against the platform's current device
  // list. Saved IDs may no longer resolve (e.g., a USB headset was unplugged),
  // and we don't want to keep applying a stale ID that the audio stack then has
  // to fall back from on every call setup.
  try {
    final (inputDevices, outputDevices) = await telepathy.listDevices();
    await audioSettingsController.pruneMissingDevices(
      inputDevices: inputDevices,
      outputDevices: outputDevices,
    );
  } catch (e, st) {
    DebugConsole.debug('Failed to reconcile audio devices on startup: $e\n$st');
  }

  // Use the post-prune controller state as the single source of truth for
  // output-device routing. Reapply it to the sound player so a cleared or
  // changed saved selection reaches the player alongside Telepathy.
  final postPruneOutputDeviceId = audioSettingsController.outputDeviceId;
  if (prePruneOutputDeviceId != postPruneOutputDeviceId) {
    soundPlayer.updateOutputDevice(deviceId: postPruneOutputDeviceId);
  }

  // apply options to the instance
  telepathy.setRmsThreshold(decimal: audioSettingsController.inputSensitivity);
  telepathy.setInputVolume(decibel: audioSettingsController.inputVolume);
  telepathy.setOutputVolume(decibel: audioSettingsController.outputVolume);
  telepathy.setDenoise(denoise: audioSettingsController.useDenoise);
  telepathy.setPlayCustomRingtones(
      play: preferencesController.playCustomRingtones);
  telepathy.setInputDevice(deviceId: audioSettingsController.inputDeviceId);
  telepathy.setOutputDevice(deviceId: postPruneOutputDeviceId);
  telepathy.setSendCustomRingtone(
      send: preferencesController.customRingtoneFile != null);
  telepathy.setEfficiencyMode(enabled: preferencesController.efficiencyMode);

  if (audioSettingsController.denoiseModel != null) {
    updateDenoiseModel(audioSettingsController.denoiseModel, telepathy);
  }

  final InterfaceController interfaceController =
      InterfaceController(options: options);
  await interfaceController.init();

  runApp(
    MultiProvider(
      providers: [
        ChangeNotifierProvider.value(value: profilesController),
        ChangeNotifierProvider.value(value: audioSettingsController),
        ChangeNotifierProvider.value(value: networkSettingsController),
        ChangeNotifierProvider.value(value: preferencesController),
        ChangeNotifierProvider.value(value: interfaceController),
        ChangeNotifierProvider.value(value: stateController),
        ChangeNotifierProvider.value(value: statisticsController),
        ChangeNotifierProvider.value(value: chatStateController),
        ChangeNotifierProvider.value(value: audioDevices),
        Provider.value(value: telepathy),
        Provider.value(value: soundPlayer),
        Provider.value(value: overlay),
      ],
      child: const TelepathyApp(),
    ),
  );
}
