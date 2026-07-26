import 'dart:async';
import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:shared_preferences_platform_interface/in_memory_shared_preferences_async.dart';
import 'package:shared_preferences_platform_interface/shared_preferences_async_platform_interface.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/core/rust/player.dart';
import 'package:telepathy/core/rust/lib.dart';
import 'package:telepathy/core/rust/flutter.dart';
import 'package:telepathy/core/rust/types.dart';
import 'package:telepathy/screens/settings/sections/audio_settings.dart';
import 'package:telepathy/widgets/common/index.dart';

void main() {
  setUp(() {
    SharedPreferencesAsyncPlatform.instance =
        InMemorySharedPreferencesAsync.empty();
  });

  tearDown(() {
    SharedPreferencesAsyncPlatform.instance = null;
  });

  testWidgets(
      'Sound Test shows the audio error dialog and clears inAudioTest after a DartError',
      (WidgetTester tester) async {
    final telepathy = _FakeTelepathy();
    final stateController = StateController();
    final audioSettingsController = _FakeAudioSettingsController();

    final audioDevices = _FakeAudioDevices(telepathy: telepathy);
    final preferencesController = _FakePreferencesController();
    final networkSettingsController = _FakeNetworkSettingsController();

    await tester.pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<StateController>.value(value: stateController),
          ChangeNotifierProvider<AudioSettingsController>.value(
            value: audioSettingsController,
          ),
          ChangeNotifierProvider<PreferencesController>.value(
            value: preferencesController,
          ),
          ChangeNotifierProvider<NetworkSettingsController>.value(
            value: networkSettingsController,
          ),
          ChangeNotifierProvider<AudioDevices>.value(value: audioDevices),
          ChangeNotifierProvider<StatisticsController>.value(
            value: StatisticsController(),
          ),
          Provider<Telepathy>.value(value: telepathy),
          Provider<SoundPlayer>.value(value: _FakeSoundPlayer()),
        ],
        child: const MaterialApp(
          home: Scaffold(
            body: SingleChildScrollView(
              child: AudioSettings(
                constraints: BoxConstraints(maxWidth: 800, maxHeight: 2000),
              ),
            ),
          ),
        ),
      ),
    );

    await tester.tap(find.widgetWithText(Button, 'Sound Test'));
    await tester.pump();

    expect(find.widgetWithText(Button, 'End Test'), findsOneWidget);
    expect(stateController.inAudioTest, isTrue);

    telepathy.completeAudioTestError();
    await tester.pumpAndSettle();

    expect(find.text('Error in Audio Test'), findsOneWidget);
    expect(find.text('microphone unavailable'), findsOneWidget);
    expect(find.widgetWithText(Button, 'Sound Test'), findsOneWidget);
    expect(find.widgetWithText(Button, 'End Test'), findsNothing);
    expect(stateController.inAudioTest, isFalse);
  });

  testWidgets('does not warn before first refresh or for available selections',
      (WidgetTester tester) async {
    final audioDevices = _FakeAudioDevices(telepathy: _FakeTelepathy());
    final settings = _FakeAudioSettingsController()
      ..inputDeviceId = 'input-1'
      ..outputDeviceId = null;

    await _pumpAudioSettings(tester, audioDevices, settings);
    expect(find.text('Selected input device is unavailable'), findsNothing);
    expect(find.text('Selected output device is unavailable'), findsNothing);

    audioDevices.publish(
      inputDevices: const [AudioDevice(name: 'Desk Mic', id: 'input-1')],
      outputDevices: const [AudioDevice(name: 'Speakers', id: 'output-1')],
      hasLoadedDevices: true,
    );
    await tester.pump();

    expect(find.text('Selected input device is unavailable'), findsNothing);
    expect(find.text('Selected output device is unavailable'), findsNothing);
  });

  testWidgets('warns independently when selected devices disappear',
      (WidgetTester tester) async {
    final audioDevices = _FakeAudioDevices(telepathy: _FakeTelepathy());
    final settings = _FakeAudioSettingsController()
      ..inputDeviceId = 'input-missing'
      ..outputDeviceId = 'output-missing';

    await _pumpAudioSettings(tester, audioDevices, settings);
    audioDevices.publish(hasLoadedDevices: true);
    await tester.pump();

    expect(find.text('Selected input device is unavailable'), findsOneWidget);
    expect(find.text('Selected output device is unavailable'), findsOneWidget);
  });

  testWidgets('removes warning after selecting an available device or Default',
      (WidgetTester tester) async {
    final audioDevices = _FakeAudioDevices(telepathy: _FakeTelepathy());
    final settings = _FakeAudioSettingsController()
      ..inputDeviceId = 'missing-input'
      ..outputDeviceId = 'missing-output';

    await _pumpAudioSettings(tester, audioDevices, settings);
    audioDevices.publish(
      inputDevices: const [AudioDevice(name: 'Desk Mic', id: 'input-1')],
      hasLoadedDevices: true,
    );
    await tester.pump();

    final dropdowns = tester.widgetList<DropDown<dynamic>>(
      find.byType(DropDown<dynamic>),
    );
    dropdowns.first.onSelected('input-1');
    dropdowns.elementAt(1).onSelected('');
    await tester.pump();

    expect(find.text('Selected input device is unavailable'), findsNothing);
    expect(find.text('Selected output device is unavailable'), findsNothing);
  });

  testWidgets('removes warning when original device reappears',
      (WidgetTester tester) async {
    final audioDevices = _FakeAudioDevices(telepathy: _FakeTelepathy());
    final settings = _FakeAudioSettingsController()..inputDeviceId = 'input-1';

    await _pumpAudioSettings(tester, audioDevices, settings);
    audioDevices.publish(hasLoadedDevices: true);
    await tester.pump();
    expect(find.text('Selected input device is unavailable'), findsOneWidget);

    audioDevices.publish(
      inputDevices: const [AudioDevice(name: 'Desk Mic', id: 'input-1')],
    );
    await tester.pump();

    expect(find.text('Selected input device is unavailable'), findsNothing);
    expect(
      tester
          .widgetList<EditableText>(find.byType(EditableText))
          .any((field) => field.controller.text == 'Desk Mic'),
      isTrue,
    );
    expect(settings.inputDeviceId, 'input-1');
  });
}

Future<void> _pumpAudioSettings(
  WidgetTester tester,
  _FakeAudioDevices audioDevices,
  _FakeAudioSettingsController audioSettingsController,
) {
  final telepathy = audioDevices.telepathy as _FakeTelepathy;
  return tester.pumpWidget(
    MultiProvider(
      providers: [
        ChangeNotifierProvider<StateController>.value(value: StateController()),
        ChangeNotifierProvider<AudioSettingsController>.value(
          value: audioSettingsController,
        ),
        ChangeNotifierProvider<PreferencesController>.value(
          value: _FakePreferencesController(),
        ),
        ChangeNotifierProvider<NetworkSettingsController>.value(
          value: _FakeNetworkSettingsController(),
        ),
        ChangeNotifierProvider<AudioDevices>.value(value: audioDevices),
        ChangeNotifierProvider<StatisticsController>.value(
          value: StatisticsController(),
        ),
        Provider<Telepathy>.value(value: telepathy),
        Provider<SoundPlayer>.value(value: _FakeSoundPlayer()),
      ],
      child: const MaterialApp(
        home: Scaffold(
          body: SingleChildScrollView(
            child: AudioSettings(
              constraints: BoxConstraints(maxWidth: 800, maxHeight: 2000),
            ),
          ),
        ),
      ),
    ),
  );
}

class _FakeAudioDevices extends AudioDevices {
  List<AudioDevice> _inputDevices = [];
  List<AudioDevice> _outputDevices = [];
  bool _hasLoadedDevices = false;

  _FakeAudioDevices({required super.telepathy});

  @override
  List<AudioDevice> get inputDevices =>
      [const AudioDevice(name: 'Default', id: ''), ..._inputDevices];

  @override
  List<AudioDevice> get outputDevices =>
      [const AudioDevice(name: 'Default', id: ''), ..._outputDevices];

  @override
  bool get hasLoadedDevices => _hasLoadedDevices;

  void publish({
    List<AudioDevice>? inputDevices,
    List<AudioDevice>? outputDevices,
    bool? hasLoadedDevices,
  }) {
    _inputDevices = inputDevices ?? _inputDevices;
    _outputDevices = outputDevices ?? _outputDevices;
    _hasLoadedDevices = hasLoadedDevices ?? _hasLoadedDevices;
    notifyListeners();
  }

  @override
  void pauseUpdates() {}

  @override
  void startUpdates() {}
}

class _FakeAudioSettingsController extends AudioSettingsController {
  _FakeAudioSettingsController() : super(options: SharedPreferencesAsync()) {
    outputVolume = 0;
    inputVolume = 0;
    soundVolume = -10;
    inputSensitivity = -16;
    useDenoise = true;
    denoiseModel = 'Vanilla';
    outputDeviceId = null;
    inputDeviceId = null;
  }
}

class _FakePreferencesController extends PreferencesController {
  _FakePreferencesController() : super(options: SharedPreferencesAsync()) {
    playCustomRingtones = true;
    customRingtoneFile = null;
    efficiencyMode = false;
  }
}

class _FakeNetworkSettingsController extends NetworkSettingsController {
  _FakeNetworkSettingsController() : super(options: SharedPreferencesAsync()) {
    codecConfig = _FakeCodecConfig();
  }
}

class _FakeCodecConfig implements CodecConfig {
  bool _enabled = true;
  bool _vbr = true;
  double _residualBits = 5.0;

  @override
  void dispose() {}

  @override
  bool get isDisposed => false;

  @override
  void setEnabled({required bool enabled}) {
    _enabled = enabled;
  }

  @override
  void setResidualBits({required double residualBits}) {
    _residualBits = residualBits;
  }

  @override
  void setVbr({required bool vbr}) {
    _vbr = vbr;
  }

  @override
  (bool, bool, double) toValues() => (_enabled, _vbr, _residualBits);
}

class _FakeSoundPlayer implements SoundPlayer {
  @override
  bool get isDisposed => false;

  @override
  void dispose() {}

  @override
  ArcHost host() {
    throw UnimplementedError();
  }

  @override
  Future<FlutterSoundHandle> play({required List<int> bytes}) {
    throw UnimplementedError();
  }

  @override
  Future<void> updateOutputDevice({String? deviceId}) async {}

  @override
  void updateOutputVolume({required double volume}) {}
}

class _FakeTelepathy implements Telepathy {
  final Completer<void> _audioTestCompleter = Completer<void>();

  @override
  bool get isDisposed => false;

  @override
  void dispose() {}

  @override
  Future<void> audioTest() => _audioTestCompleter.future;

  void completeAudioTestError() {
    _audioTestCompleter.completeError(
      const DartError(message: 'microphone unavailable'),
    );
  }

  @override
  ChatMessage buildChat({
    required Contact contact,
    required String text,
    required List<(String, Uint8List)> attachments,
  }) {
    throw UnimplementedError();
  }

  @override
  Future<void> endCall() async {}

  @override
  Future<void> joinRoom({
    required List<String> memberStrings,
    required StartOperation operation,
  }) async {}

  @override
  Future<(List<AudioDevice>, List<AudioDevice>)> listDevices() async =>
      (<AudioDevice>[], <AudioDevice>[]);

  @override
  StartOperation newStartOperation() => throw UnimplementedError();

  @override
  void pauseStatistics() {}

  @override
  Future<void> restartManager() async {}

  @override
  Future<PreparedIdentitySwitch> prepareIdentitySwitch({
    required List<int> targetKey,
    required List<Contact> targetContacts,
  }) async =>
      throw UnimplementedError();

  @override
  void resumeStatistics() {}

  @override
  Future<void> sendChat({required ChatMessage message}) async {}

  @override
  void setContactOutputVolume({required Contact contact}) {}

  @override
  void setDeafened({required bool deafened}) {}

  @override
  void setDenoise({required bool denoise}) {}

  @override
  void setEfficiencyMode({required bool enabled}) {}

  @override
  Future<void> setIdentity({required List<int> key}) async {}

  @override
  Future<void> setInputDevice({String? deviceId}) async {}

  @override
  void setInputVolume({required double decibel}) {}

  @override
  Future<void> setModel({Uint8List? model}) async {}

  @override
  void setMuted({required bool muted}) {}

  @override
  Future<void> setOutputDevice({String? deviceId}) async {}

  @override
  void setOutputVolume({required double decibel}) {}

  @override
  void setPlayCustomRingtones({required bool play}) {}

  @override
  void setRmsThreshold({required double decimal}) {}

  @override
  void setSendCustomRingtone({required bool send}) {}

  @override
  Future<void> shutdown() async {}

  @override
  Future<void> startCall({
    required Contact contact,
    required StartOperation operation,
  }) async {}

  @override
  Future<void> startManager() async {}

  @override
  Future<void> startScreenshare({required Contact contact}) async {}

  @override
  Future<void> startSession({required Contact contact}) async {}

  @override
  Future<void> stopSession({required Contact contact}) async {}
}
