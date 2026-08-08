import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:flutter/material.dart' hide Overlay;
import 'package:flutter_driver/driver_extension.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:shared_preferences_platform_interface/in_memory_shared_preferences_async.dart';
import 'package:shared_preferences_platform_interface/shared_preferences_async_platform_interface.dart';
import 'package:telepathy/app.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/core/rust/flutter.dart';
import 'package:telepathy/core/rust/player.dart';
import 'package:telepathy/core/testing/mock_backend.dart';
import 'package:window_manager/window_manager.dart';

/// The active mock scenario, set with
/// `--dart-define=MOCK_SCENARIO=<demo|room-active|empty>`.
const String mockScenario =
    String.fromEnvironment('MOCK_SCENARIO', defaultValue: 'demo');

/// Mock-mode entrypoint: boots the real app UI against the fake backend in
/// `lib/core/testing/mock_backend.dart` — no Rust core, no network, seeded
/// demo data. Used for visual iteration and headless QA (see
/// `scripts/run-linux-debug.sh`, which selects this target via `TARGET`).
Future<void> main(List<String> args) async {
  enableFlutterDriverExtension();
  WidgetsFlutterBinding.ensureInitialized();
  if (!kIsWeb) {
    await windowManager.ensureInitialized();
  }

  SharedPreferencesAsyncPlatform.instance =
      InMemorySharedPreferencesAsync.empty();
  final SharedPreferencesAsync options = SharedPreferencesAsync();

  final MockAppContext mock =
      await createMockAppContext(scenario: mockScenario, options: options);

  final AudioSettingsController audioSettingsController =
      AudioSettingsController(options: options);
  await audioSettingsController.init();

  final PreferencesController preferencesController =
      PreferencesController(options: options);
  await preferencesController.init();

  final InterfaceController interfaceController =
      InterfaceController(options: options);
  await interfaceController.init();

  final StatisticsController statisticsController = StatisticsController();
  final ChatStateController chatStateController =
      ChatStateController(mock.soundPlayer);
  final AudioDevices audioDevices = AudioDevices(telepathy: mock.telepathy);

  runApp(
    MultiProvider(
      providers: [
        ChangeNotifierProvider.value(value: mock.profilesController),
        ChangeNotifierProvider.value(value: audioSettingsController),
        ChangeNotifierProvider.value(value: preferencesController),
        ChangeNotifierProvider.value(value: interfaceController),
        ChangeNotifierProvider.value(value: mock.stateController),
        ChangeNotifierProvider.value(value: statisticsController),
        ChangeNotifierProvider.value(value: chatStateController),
        ChangeNotifierProvider.value(value: audioDevices),
        Provider<Telepathy>.value(value: mock.telepathy),
        Provider<SoundPlayer>.value(value: mock.soundPlayer),
      ],
      child: const TelepathyApp(),
    ),
  );
}
