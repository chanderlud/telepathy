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
import 'package:telepathy/core/rust/types.dart';
import 'package:telepathy/models/index.dart';
import 'package:telepathy/screens/home/home_page.dart';
import 'package:telepathy/widgets/call/call_details_widget.dart';
import 'package:telepathy/widgets/contacts/contacts_list.dart';

import '../../support/fake_contact.dart';

/// Layout regression tests for https://github.com/chanderlud/telepathy/issues/58
///
/// Each case pumps [HomePage] at the canvas size from the issue's
/// screenshots. `RenderFlex` overflows and clamped children surface as
/// test failures in debug builds.

class _FakeArcHost implements ArcHost {
  @override
  void dispose() {}

  @override
  bool get isDisposed => false;
}

class _FakeSoundHandle implements FlutterSoundHandle {
  @override
  void cancel() {}

  @override
  void dispose() {}

  @override
  bool get isDisposed => false;
}

class _FakeSoundPlayer implements SoundPlayer {
  final _FakeSoundHandle _handle = _FakeSoundHandle();

  @override
  ArcHost host() => _FakeArcHost();

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

class _FakeTelepathy implements Telepathy {
  @override
  Object? noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

/// AssetBundle that resolves `.svg` requests to a minimal SVG whose viewport
/// matches the real icons in `assets/icons/` (viewBox 0 0 24 24 with 128px
/// width/height attributes — the parser derives the intrinsic 24px size from
/// the viewBox, so un-sized `SvgPicture`s measure exactly like production).
class _SvgAwareAssetBundle extends CachingAssetBundle {
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
  /// widths/line heights, which hid the overflows this suite guards
  /// against. Must be called from `setUpAll` — it performs real file I/O,
  /// which never completes inside a test body's fake async zone.
  static Future<void> loadAppFont() async {
    if (_fontLoaded) return;
    final loader = FontLoader('Nunito');
    for (final entry in const {
      'Nunito-Regular.ttf': FontWeight.w400,
      'Nunito-Medium.ttf': FontWeight.w500,
      'Nunito-SemiBold.ttf': FontWeight.w600,
      'Nunito-Bold.ttf': FontWeight.w700,
    }.entries) {
      final bytes =
          await File('assets/fonts/nunito/${entry.key}').readAsBytes();
      loader.addFont(Future.value(bytes.buffer.asByteData()));
    }
    await loader.load();
    _fontLoaded = true;
  }

  final AudioSettingsController audioSettingsController;

  Harness({
    required this.stateController,
    required this.profilesController,
    required this.audioSettingsController,
  });

  final StateController stateController;
  final ProfilesController profilesController;

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
    );
    await profilesController.init(const <String>[]);

    final stateController = StateController();
    final audioSettingsController =
        AudioSettingsController(options: SharedPreferencesAsync());
    await audioSettingsController.init();
    final harness = Harness(
      stateController: stateController,
      profilesController: profilesController,
      audioSettingsController: audioSettingsController,
    );
    return harness;
  }

  void addContact(FakeContact contact) {
    profilesController.profiles[profilesController.activeProfile]!
        .contacts[contact.id()] = contact;
  }

  void addRoom(Room room) {
    profilesController
        .profiles[profilesController.activeProfile]!.rooms[room.id] = room;
  }

  Widget buildApp() {
    return DefaultAssetBundle(
      bundle: _SvgAwareAssetBundle(),
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
              create: (_) => ChatStateController(_FakeSoundPlayer())),
          Provider<Telepathy>.value(value: _FakeTelepathy()),
          Provider<SoundPlayer>.value(value: _FakeSoundPlayer()),
        ],
        child: MaterialApp(
          // Match production text metrics (engine default on Linux/Android).
          theme: ThemeData(fontFamily: 'Nunito'),
          home: const HomePage(),
        ),
      ),
    );
  }
}

void _setCanvasSize(WidgetTester tester, Size size) {
  tester.view.devicePixelRatio = 1.0;
  tester.view.physicalSize = size;
  addTearDown(tester.view.reset);
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  setUpAll(Harness.loadAppFont);

  testWidgets(
      'case 1: semi-narrow two-column layout with an active call does '
      'not overflow the contacts area', (WidgetTester tester) async {
    final harness = await Harness.create();
    final contact = FakeContact(id: 'contact-1', contactNickname: 'Profile 2');
    harness.addContact(contact);
    harness.addRoom(
        Room(id: 'room-1', peerIds: const ['p1'], nickname: 'test room'));

    harness.stateController.updateSession((
      contact.peerId(),
      const SessionStatus.connected(relayed: true, remoteAddress: 'usw1-1'),
    ));
    harness.stateController.setPendingContact(contact);
    harness.stateController.setActiveContact(contact);
    harness.stateController.setStatus('Active');

    _setCanvasSize(tester, const Size(807, 910));
    await tester.pumpWidget(harness.buildApp());
    // Let the AnimatedSize transition finish so the steady-state layout is
    // what gets verified.
    await tester.pump(const Duration(milliseconds: 300));

    // Guard against vacuous passes: the call details card must actually be
    // on screen for the overflow checks to mean anything.
    expect(find.byType(CallDetailsWidget), findsOneWidget);

    // The end-call button must stay flush against the row's right padding —
    // flexible text slots that underfill let it float mid-row (regression).
    final cardRight = tester.getRect(find.byType(ContactsList)).right;
    final endCallButton = find.ancestor(
        of: find.bySemanticsLabel('End call icon'),
        matching: find.byType(IconButton));
    expect(endCallButton, findsWidgets);
    for (final element in tester.elementList(endCallButton)) {
      final buttonRect = tester.getRect(find.byWidget(element.widget));
      // card padding 12 + item margin 6 + item padding 10
      expect(cardRight - 28 - buttonRect.right, lessThanOrEqualTo(1.5),
          reason: 'row buttons must stay flush right');
    }
  });

  testWidgets(
      'case 2: medium height narrow layout does not clip the bottom '
      'of the call controls', (WidgetTester tester) async {
    final harness = await Harness.create();
    harness
        .addContact(FakeContact(id: 'c1', contactNickname: 'Default Profile'));
    harness.addContact(FakeContact(id: 'c2', contactNickname: 'Profile 3'));
    harness.addRoom(
        Room(id: 'room-1', peerIds: const ['p1'], nickname: 'test room'));

    _setCanvasSize(tester, const Size(605, 892));
    await tester.pumpWidget(harness.buildApp());
    await tester.pump();

    // The bottom control bar must be fully inside the canvas.
    final bottomBar = find.byIcon(Icons.call);
    expect(bottomBar, findsOneWidget);
    final Rect appRect = tester.getRect(find.byType(HomePage));
    expect(appRect.bottom, lessThanOrEqualTo(892));
  });

  testWidgets(
      'case 1b: contacts header does not overflow just above the '
      'wide breakpoint with a failed session manager',
      (WidgetTester tester) async {
    final harness = await Harness.create();
    final contact = FakeContact(
        id: 'contact-1', contactNickname: 'A Very Long Profile Nickname');
    harness.addContact(contact);
    harness.stateController.updateSession((
      contact.peerId(),
      const SessionStatus.connected(
          relayed: true, remoteAddress: 'a-quite-long-relay-address-1'),
    ));
    harness.stateController.setPendingContact(contact);
    harness.stateController.setActiveContact(contact);
    harness.stateController.setStatus('Active');
    harness.stateController.setSessionManager(ManagerState.failed);

    // Narrow two-column layout: the contacts card is at its tightest.
    // (The default flutter_test font has wider metrics than the bundled
    // UI font; 700px here exercises the same stress as ~660px in the app.)
    _setCanvasSize(tester, const Size(700, 900));
    await tester.pumpWidget(harness.buildApp());
    // Let the AnimatedSize transition finish so the steady-state layout is
    // what gets verified.
    await tester.pump(const Duration(milliseconds: 300));

    // Guard against vacuous passes: the call details card must actually be
    // on screen for the overflow checks to mean anything.
    expect(find.byType(CallDetailsWidget), findsOneWidget);

    // The end-call button must stay flush against the row's right padding —
    // flexible text slots that underfill let it float mid-row (regression).
    final cardRight = tester.getRect(find.byType(ContactsList)).right;
    final endCallButton = find.ancestor(
        of: find.bySemanticsLabel('End call icon'),
        matching: find.byType(IconButton));
    expect(endCallButton, findsWidgets);
    for (final element in tester.elementList(endCallButton)) {
      final buttonRect = tester.getRect(find.byWidget(element.widget));
      // card padding 12 + item margin 6 + item padding 10
      expect(cardRight - 28 - buttonRect.right, lessThanOrEqualTo(1.5),
          reason: 'row buttons must stay flush right');
    }
  });

  testWidgets(
      'case 2b: compact height narrow layout with an active call '
      'does not overflow', (WidgetTester tester) async {
    final harness = await Harness.create();
    final contact = FakeContact(id: 'c1', contactNickname: 'Profile 3');
    harness.addContact(contact);
    harness.stateController.setPendingContact(contact);
    harness.stateController.setActiveContact(contact);
    harness.stateController.setStatus('Active');

    _setCanvasSize(tester, const Size(605, 600));
    await tester.pumpWidget(harness.buildApp());
    // Let the AnimatedSize transition finish so the steady-state layout is
    // what gets verified.
    await tester.pump(const Duration(milliseconds: 300));
  });

  testWidgets(
      'case 3: connecting spinner keeps its size in a compact height '
      'layout', (WidgetTester tester) async {
    final harness = await Harness.create();
    final contact = FakeContact(id: 'c1', contactNickname: 'Profile 3');
    harness.addContact(contact);
    harness.stateController
        .updateSession((contact.peerId(), const SessionStatus.connecting()));

    _setCanvasSize(tester, const Size(807, 620));
    await tester.pumpWidget(harness.buildApp());
    await tester.pump();

    final spinner = find.byType(CircularProgressIndicator);
    expect(spinner, findsWidgets);
    for (final element in tester.elementList(spinner)) {
      final size = tester.getSize(find.byWidget(element.widget));
      expect(size.height, greaterThanOrEqualTo(18),
          reason: 'the session spinner must not be squished vertically');
    }
  });

  testWidgets(
      'case 4: active call widgets do not overflow vertically on a '
      'short layout', (WidgetTester tester) async {
    final harness = await Harness.create();
    final contact = FakeContact(id: 'c1', contactNickname: 'Profile 3');
    harness.addContact(contact);
    harness.stateController.updateSession((
      contact.peerId(),
      const SessionStatus.connected(relayed: true, remoteAddress: 'usw1-1'),
    ));
    harness.stateController.setPendingContact(contact);
    harness.stateController.setActiveContact(contact);
    harness.stateController.setStatus('Active');

    _setCanvasSize(tester, const Size(1002, 848));
    await tester.pumpWidget(harness.buildApp());
    // Let the AnimatedSize transition finish so the steady-state layout is
    // what gets verified.
    await tester.pump(const Duration(milliseconds: 300));

    // Guard against vacuous passes: the call details card must actually be
    // on screen for the overflow checks to mean anything.
    expect(find.byType(CallDetailsWidget), findsOneWidget);

    // The end-call button must stay flush against the row's right padding —
    // flexible text slots that underfill let it float mid-row (regression).
    final cardRight = tester.getRect(find.byType(ContactsList)).right;
    final endCallButton = find.ancestor(
        of: find.bySemanticsLabel('End call icon'),
        matching: find.byType(IconButton));
    expect(endCallButton, findsWidgets);
    for (final element in tester.elementList(endCallButton)) {
      final buttonRect = tester.getRect(find.byWidget(element.widget));
      // card padding 12 + item margin 6 + item padding 10
      expect(cardRight - 28 - buttonRect.right, lessThanOrEqualTo(1.5),
          reason: 'row buttons must stay flush right');
    }
  });

  testWidgets(
      'case 3b: connecting spinner keeps its size at the narrow '
      'layout breakpoint', (WidgetTester tester) async {
    // 620px canvas: the narrow branch is chosen from the padded width (580)
    // while MediaQuery still reports a "wide" 620 — a mismatch that used to
    // apply the wrong cap here (see home_page.dart). The 250px cap split
    // across 3 items left the spinner row only 34px tall.
    final harness = await Harness.create();
    final contact = FakeContact(id: 'c1', contactNickname: 'Profile 3');
    harness.addContact(contact);
    harness
        .addContact(FakeContact(id: 'c2', contactNickname: 'Default Profile'));
    harness.stateController
        .updateSession((contact.peerId(), const SessionStatus.connecting()));

    _setCanvasSize(tester, const Size(620, 800));
    await tester.pumpWidget(harness.buildApp());
    await tester.pump();

    final spinner = find.byType(CircularProgressIndicator);
    expect(spinner, findsWidgets);
    for (final element in tester.elementList(spinner)) {
      final size = tester.getSize(find.byWidget(element.widget));
      expect(size, const Size(20, 20),
          reason: 'the session spinner must not be squished');
    }
  });

  testWidgets(
      'case 2c: short narrow layout at the width breakpoint does '
      'not overflow the call controls', (WidgetTester tester) async {
    // 620x400: the padded/MediaQuery width mismatch applied the 250px
    // contacts cap in a compact-height window, crushing the tab view and
    // overflowing CallControls by 46px.
    final harness = await Harness.create();
    final contact = FakeContact(id: 'c1', contactNickname: 'Profile 3');
    harness.addContact(contact);
    harness.stateController.setPendingContact(contact);
    harness.stateController.setActiveContact(contact);
    harness.stateController.setStatus('Active');

    _setCanvasSize(tester, const Size(620, 400));
    await tester.pumpWidget(harness.buildApp());
    // Let the AnimatedSize transition finish so the steady-state layout is
    // what gets verified.
    await tester.pump(const Duration(milliseconds: 300));
  });

  testWidgets(
      'case 4b: compact height wide layout with an active call does '
      'not overflow', (WidgetTester tester) async {
    final harness = await Harness.create();
    final contact = FakeContact(id: 'c1', contactNickname: 'Profile 3');
    harness.addContact(contact);
    harness.stateController.setPendingContact(contact);
    harness.stateController.setActiveContact(contact);
    harness.stateController.setStatus('Active');

    _setCanvasSize(tester, const Size(1002, 620));
    await tester.pumpWidget(harness.buildApp());
    // Let the AnimatedSize transition finish so the steady-state layout is
    // what gets verified.
    await tester.pump(const Duration(milliseconds: 300));

    // Guard against vacuous passes: the call details card must actually be
    // on screen for the overflow checks to mean anything.
    expect(find.byType(CallDetailsWidget), findsOneWidget);

    // The end-call button must stay flush against the row's right padding —
    // flexible text slots that underfill let it float mid-row (regression).
    final cardRight = tester.getRect(find.byType(ContactsList)).right;
    final endCallButton = find.ancestor(
        of: find.bySemanticsLabel('End call icon'),
        matching: find.byType(IconButton));
    expect(endCallButton, findsWidgets);
    for (final element in tester.elementList(endCallButton)) {
      final buttonRect = tester.getRect(find.byWidget(element.widget));
      // card padding 12 + item margin 6 + item padding 10
      expect(cardRight - 28 - buttonRect.right, lessThanOrEqualTo(1.5),
          reason: 'row buttons must stay flush right');
    }
  });
}
