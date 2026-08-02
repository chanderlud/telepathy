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

    final result = await acceptCallPrompt(
      navigatorKey.currentState!.context,
      contact,
      cancel.notified(),
    );

    handle?.cancel();

    if (result) {
      // Move through the same connecting->active gate the outgoing
      // path uses.
      stateController.setStatus('Connecting');
      stateController.setPendingContact(contact);
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
        // Distinguish a first promotion (connecting -> active) from an
        // already-active room (e.g. after a `Waiting` event promoted the
        // pending slot first). Stale callbacks for idle/ending/attempts are
        // rejected. The start future can resolve after this callback, so it
        // must not own promotion.
        if (!stateController
            .handleConnectedEvent(stateController.currentCallAttempt)) {
          return;
        }
        bytes = await readSeaBytes('connected');
        if (stateController.callLifecycle != CallLifecycle.active) {
          return;
        }
      case CallState_Waiting():
        // Rooms can wait for peers without a Connected callback. Promote the
        // pending room so its normal call controls, including hangup, render.
        stateController
            .promotePendingCallAttempt(stateController.currentCallAttempt);
        stateController.setStatus('Waiting for peers');
        return;
      case CallState_RoomJoin():
        stateController.roomJoin(state.field0);
        return; // TODO add room join sound
      case CallState_RoomLeave():
        stateController.roomLeave(state.field0);
        return; // TODO add room leave sound
      case CallState_CallEnded():
        // Local hangup remains in `ending` until endCall confirms backend slot
        // release. Its trailing event must not clear that gate early.
        final localHangup =
            stateController.callLifecycle == CallLifecycle.ending;
        if (localHangup) {
          return;
        }
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

  // apply options to the instance
  telepathy.setRmsThreshold(decimal: audioSettingsController.inputSensitivity);
  telepathy.setInputVolume(decibel: audioSettingsController.inputVolume);
  telepathy.setOutputVolume(decibel: audioSettingsController.outputVolume);
  telepathy.setDenoise(denoise: audioSettingsController.useDenoise);
  telepathy.setPlayCustomRingtones(
      play: preferencesController.playCustomRingtones);
  telepathy.setInputDevice(deviceId: audioSettingsController.inputDeviceId);
  telepathy.setOutputDevice(deviceId: audioSettingsController.outputDeviceId);
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
