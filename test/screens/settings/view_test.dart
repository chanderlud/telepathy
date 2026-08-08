import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:shared_preferences_platform_interface/in_memory_shared_preferences_async.dart';
import 'package:shared_preferences_platform_interface/shared_preferences_async_platform_interface.dart';
import 'package:telepathy/controllers/audio_devices_controller.dart';
import 'package:telepathy/controllers/audio_settings_controller.dart';
import 'package:telepathy/controllers/network_settings_controller.dart';
import 'package:telepathy/controllers/preferences_controller.dart';
import 'package:telepathy/controllers/state_controller.dart';
import 'package:telepathy/controllers/statistics_controller.dart';
import 'package:telepathy/core/rust/flutter.dart';
import 'package:telepathy/core/rust/lib.dart';
import 'package:telepathy/core/rust/player.dart';
import 'package:telepathy/core/rust/types.dart';
import 'package:telepathy/screens/settings/header.dart';
import 'package:telepathy/screens/settings/menu.dart';
import 'package:telepathy/screens/settings/view.dart';

void main() {
  setUp(() {
    SharedPreferencesAsyncPlatform.instance =
        InMemorySharedPreferencesAsync.empty();
  });

  tearDown(() {
    SharedPreferencesAsyncPlatform.instance = null;
  });

  testWidgets('narrow menu dismisses on outside interaction',
      (WidgetTester tester) async {
    await _pumpSettingsPage(tester, maxWidth: 500);

    await _openMenu(tester);
    expect(find.byType(SettingsMenu), findsOneWidget);

    await tester.tapAt(const Offset(850, 800));
    await tester.pump();

    expect(find.byType(SettingsMenu), findsNothing);
  });

  testWidgets('narrow menu dismisses when focus leaves it',
      (WidgetTester tester) async {
    final outsideFocusNode = FocusNode();
    addTearDown(outsideFocusNode.dispose);
    await _pumpSettingsPage(
      tester,
      maxWidth: 500,
      outsideFocusNode: outsideFocusNode,
    );

    await _openMenu(tester);
    expect(find.byType(SettingsMenu), findsOneWidget);

    outsideFocusNode.requestFocus();
    await tester.pumpAndSettle();

    expect(outsideFocusNode.hasFocus, isTrue);
    expect(find.byType(SettingsMenu), findsNothing);
  });

  testWidgets('narrow menu keeps header and internal menu interactions',
      (WidgetTester tester) async {
    await _pumpSettingsPage(tester, maxWidth: 500);

    await _openMenu(tester);
    await tester.tap(_menuButton);
    await tester.pump();
    expect(find.byType(SettingsMenu), findsNothing);

    await _openMenu(tester);
    await tester.tap(find.text('Audio & Video'));
    await tester.pump();

    expect(find.byType(SettingsMenu), findsOneWidget);
  });

  testWidgets('wide menu stays visible after outside interaction',
      (WidgetTester tester) async {
    await _pumpSettingsPage(tester, maxWidth: 700);
    expect(find.byType(SettingsMenu), findsOneWidget);

    await tester.tapAt(const Offset(850, 800));
    await tester.pump();

    expect(find.byType(SettingsMenu), findsOneWidget);
  });
}

final Finder _menuButton = find
    .descendant(
      of: find.byType(SettingsHeader),
      matching: find.byType(IconButton),
    )
    .last;

Future<void> _openMenu(WidgetTester tester) async {
  await tester.tap(_menuButton);
  await tester.pumpAndSettle();
}

Future<void> _pumpSettingsPage(
  WidgetTester tester, {
  required double maxWidth,
  FocusNode? outsideFocusNode,
}) async {
  tester.view.devicePixelRatio = 1;
  tester.view.physicalSize = const Size(900, 900);
  addTearDown(tester.view.reset);

  final options = SharedPreferencesAsync();
  final audioSettingsController = AudioSettingsController(options: options);
  await audioSettingsController.init();
  final preferencesController = PreferencesController(options: options);
  await preferencesController.init();
  final telepathy = _FakeTelepathy();

  final page = Scaffold(
    body: SettingsPage(
      constraints: BoxConstraints(maxWidth: maxWidth, maxHeight: 900),
    ),
  );
  final home = outsideFocusNode == null
      ? page
      : Row(
          children: [
            Expanded(child: page),
            Focus(
              focusNode: outsideFocusNode,
              child: const SizedBox(width: 1, height: 1),
            ),
          ],
        );

  await tester.pumpWidget(
    MultiProvider(
      providers: [
        ChangeNotifierProvider<StateController>.value(value: StateController()),
        ChangeNotifierProvider<AudioSettingsController>.value(
          value: audioSettingsController,
        ),
        ChangeNotifierProvider<PreferencesController>.value(
          value: preferencesController,
        ),
        ChangeNotifierProvider<NetworkSettingsController>.value(
          value: _FakeNetworkSettingsController(options: options),
        ),
        ChangeNotifierProvider<AudioDevices>.value(
          value: _FakeAudioDevices(telepathy: telepathy),
        ),
        ChangeNotifierProvider<StatisticsController>.value(
          value: StatisticsController(),
        ),
        Provider<Telepathy>.value(value: telepathy),
        Provider<SoundPlayer>.value(value: _FakeSoundPlayer()),
      ],
      child: MaterialApp(home: home),
    ),
  );
  await tester.pump();
}

class _FakeAudioDevices extends AudioDevices {
  _FakeAudioDevices({required super.telepathy});

  @override
  List<AudioDevice> get inputDevices => const [
        AudioDevice(name: 'Default', id: ''),
      ];

  @override
  List<AudioDevice> get outputDevices => const [
        AudioDevice(name: 'Default', id: ''),
      ];

  @override
  bool get hasLoadedDevices => false;

  @override
  void pauseUpdates() {}

  @override
  void startUpdates() {}
}

class _FakeNetworkSettingsController extends NetworkSettingsController {
  _FakeNetworkSettingsController({required super.options}) {
    codecConfig = _FakeCodecConfig();
    screenshareConfig = _FakeScreenshareConfig();
  }
}

class _FakeCodecConfig implements CodecConfig {
  @override
  void dispose() {}

  @override
  bool get isDisposed => false;

  @override
  void setEnabled({required bool enabled}) {}

  @override
  void setResidualBits({required double residualBits}) {}

  @override
  void setVbr({required bool vbr}) {}

  @override
  (bool, bool, double) toValues() => (true, true, 5);
}

class _FakeScreenshareConfig implements ScreenshareConfig {
  @override
  Future<Capabilities> capabilities() async => _FakeCapabilities();

  @override
  void dispose() {}

  @override
  bool get isDisposed => false;

  @override
  Future<RecordingConfig?> recordingConfig() async => null;

  @override
  Uint8List toBytes() => Uint8List(0);

  @override
  Future<void> updateRecordingConfig({
    required String encoder,
    required String device,
    required int bitrate,
    required int framerate,
    int? height,
  }) async {}
}

class _FakeCapabilities implements Capabilities {
  @override
  List<String> devices() => const [];

  @override
  void dispose() {}

  @override
  List<String> encoders() => const [];

  @override
  bool get isDisposed => false;
}

class _FakeSoundPlayer implements SoundPlayer {
  @override
  Object? noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

class _FakeTelepathy implements Telepathy {
  @override
  Object? noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}
