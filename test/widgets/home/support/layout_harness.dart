import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:shared_preferences_platform_interface/in_memory_shared_preferences_async.dart';
import 'package:shared_preferences_platform_interface/shared_preferences_async_platform_interface.dart';
import 'package:telepathy/controllers/audio_settings_controller.dart';
import 'package:telepathy/controllers/chat_controller.dart';
import 'package:telepathy/controllers/profiles_controller.dart';
import 'package:telepathy/controllers/state_controller.dart';
import 'package:telepathy/controllers/statistics_controller.dart';
import 'package:telepathy/core/rust/flutter.dart';
import 'package:telepathy/core/rust/player.dart';
import 'package:telepathy/models/index.dart';
import 'package:telepathy/screens/home/home_page.dart';

import '../../../support/fake_contact.dart';

/// Shared harness for the home-layout regression suites
/// (https://github.com/chanderlud/telepathy/issues/58).
///
/// Pumps the real [HomePage] with fake rust-bridge types at an exact canvas
/// size. `RenderFlex` overflows surface as test failures automatically.

class FakeArcHost implements ArcHost {
  @override
  void dispose() {}

  @override
  bool get isDisposed => false;
}

class FakeSoundHandle implements FlutterSoundHandle {
  @override
  void cancel() {}

  @override
  void dispose() {}

  @override
  bool get isDisposed => false;
}

class FakeSoundPlayer implements SoundPlayer {
  final FakeSoundHandle _handle = FakeSoundHandle();

  @override
  ArcHost host() => FakeArcHost();

  @override
  Future<FlutterSoundHandle> play({required List<int> bytes}) async => _handle;

  @override
  Future<void> updateOutputDevice({String? deviceId}) async {}

  @override
  void updateOutputVolume({required double volume}) {}

  @override
  void dispose() {}

  @override
  bool get isDisposed => false;
}

class FakeTelepathy implements Telepathy {
  @override
  Object? noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

/// AssetBundle that resolves `.svg` requests to a minimal SVG whose viewport
/// matches the real icons in `assets/icons/` (viewBox 0 0 24 24 with 128px
/// width/height attributes — the parser derives the intrinsic 24px size from
/// the viewBox, so un-sized `SvgPicture`s measure exactly like production).
class SvgAwareAssetBundle extends CachingAssetBundle {
  @override
  Future<ByteData> load(String key) async {
    if (key.endsWith('.svg')) {
      const svg = '<svg viewBox="0 0 24 24" width="128" height="128"></svg>';
      final Uint8List bytes = Uint8List.fromList(utf8.encode(svg));
      return bytes.buffer.asByteData();
    }
    return ByteData(0);
  }
}

class Harness {
  static bool _fontLoaded = false;

  /// Loads the app's bundled font so text metrics match production; the
  /// flutter_test default font has significantly different glyph
  /// widths/line heights, which hid the overflows these suites guard
  /// against. Must be called from `setUpAll` — it performs real file I/O,
  /// which never completes inside a test body's fake async zone.
  static Future<void> loadAppFont() async {
    if (_fontLoaded) return;
    final loader = FontLoader('Nunito');
    for (final name in const [
      'Nunito-Regular.ttf',
      'Nunito-Medium.ttf',
      'Nunito-SemiBold.ttf',
      'Nunito-Bold.ttf',
    ]) {
      final bytes = await File('assets/fonts/nunito/$name').readAsBytes();
      loader.addFont(Future.value(bytes.buffer.asByteData()));
    }
    await loader.load();
    _fontLoaded = true;
  }

  final StateController stateController;
  final ProfilesController profilesController;
  final AudioSettingsController audioSettingsController;

  Harness({
    required this.stateController,
    required this.profilesController,
    required this.audioSettingsController,
  });

  static Future<Harness> create() async {
    FlutterSecureStorage.setMockInitialValues(<String, String>{});
    SharedPreferencesAsyncPlatform.instance =
        InMemorySharedPreferencesAsync.empty();

    const String profileId = 'layout-test-profile';
    const FlutterSecureStorage storage = FlutterSecureStorage();
    final options = SharedPreferencesAsync();

    await storage.write(
      key: '$profileId-keypair',
      value: base64Encode(List<int>.generate(32, (int index) => index)),
    );
    await storage.write(key: '$profileId-peerId', value: 'layout-peer-id');
    await storage.write(key: '$profileId-nickname', value: 'Layout User');
    await storage.write(key: '$profileId-contacts', value: '{}');
    await storage.write(key: '$profileId-rooms', value: '{}');
    await options.setStringList('profilesV2', const <String>[profileId]);
    await options.setString('activeProfile', profileId);

    final profilesController = ProfilesController(
      storage: storage,
      options: options,
      roomHasher: ({required List<String> peers}) => peers.join('|'),
      contactFactory: ({required nickname, required peerId}) =>
          FakeContact(id: peerId, contactNickname: nickname),
      peerIdValidator: (String peerId) => peerId.trim().isNotEmpty,
    );
    await profilesController.init(const <String>[]);

    final audioSettingsController =
        AudioSettingsController(options: SharedPreferencesAsync());
    await audioSettingsController.init();

    return Harness(
      stateController: StateController(),
      profilesController: profilesController,
      audioSettingsController: audioSettingsController,
    );
  }

  void addContact(dynamic contact) {
    profilesController.profiles[profilesController.activeProfile]!
        .contacts[contact.id()] = contact;
  }

  void addRoom(Room room) {
    profilesController
        .profiles[profilesController.activeProfile]!.rooms[room.id] = room;
  }

  /// Puts the given contact into a live call through the real lifecycle
  /// path (pending → active), so `hasLiveCall` is true.
  void startCallWith(dynamic contact) {
    stateController.setPendingContact(contact);
    stateController.setActiveContact(contact);
    stateController.setStatus('Active');
  }

  Widget buildApp() {
    return DefaultAssetBundle(
      bundle: SvgAwareAssetBundle(),
      child: MultiProvider(
        providers: [
          ChangeNotifierProvider<StateController>.value(value: stateController),
          ChangeNotifierProvider<ProfilesController>.value(
              value: profilesController),
          ChangeNotifierProvider<AudioSettingsController>.value(
              value: audioSettingsController),
          ChangeNotifierProvider<StatisticsController>(
              create: (_) => StatisticsController()),
          ChangeNotifierProvider<ChatStateController>(
              create: (_) => ChatStateController(FakeSoundPlayer())),
          Provider<Telepathy>.value(value: FakeTelepathy()),
          Provider<SoundPlayer>.value(value: FakeSoundPlayer()),
        ],
        child: MaterialApp(
          // Match production text metrics (bundled app font).
          theme: ThemeData(fontFamily: 'Nunito'),
          home: const HomePage(),
        ),
      ),
    );
  }
}

void setCanvasSize(WidgetTester tester, Size size) {
  tester.view.devicePixelRatio = 1.0;
  tester.view.physicalSize = size;
  addTearDown(tester.view.reset);
}

/// Pumps the app and lets the AnimatedSize transition finish so the
/// steady-state layout is what gets verified.
Future<void> pumpApp(WidgetTester tester, Harness harness) async {
  await tester.pumpWidget(harness.buildApp());
  await tester.pump(const Duration(milliseconds: 300));
}
