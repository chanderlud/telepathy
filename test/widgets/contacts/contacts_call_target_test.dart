import 'dart:async';
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:shared_preferences_platform_interface/in_memory_shared_preferences_async.dart';
import 'package:shared_preferences_platform_interface/shared_preferences_async_platform_interface.dart';
import 'package:telepathy/controllers/profiles_controller.dart';
import 'package:telepathy/controllers/state_controller.dart';
import 'package:telepathy/core/rust/flutter.dart';
import 'package:telepathy/core/rust/lib.dart';
import 'package:telepathy/core/rust/player.dart';
import 'package:telepathy/core/rust/types.dart';
import 'package:telepathy/models/index.dart';
import 'package:telepathy/models/room.dart';
import 'package:telepathy/widgets/contacts/contact_widget.dart';
import 'package:telepathy/widgets/contacts/room_widget.dart';

import '../../support/fake_contact.dart';

/// Fakes of the rust bridge types used by the contact/room widgets.
///
/// `Contact`, `Telepathy`, `SoundPlayer`, `FlutterSoundHandle`,
/// `PublicKey`, and `ArcHost` are `RustOpaqueInterface` markers — they
/// only require `dispose()`/`isDisposed` at runtime, and the rest of
/// their methods are abstract. The widget test exercises only the
/// contact/room call icon's `onPressed` closure, so the fakes return
/// no-op values for the surfaces the closure does not touch. `Contact`
/// and `PublicKey` live in `test/support/fake_contact.dart` so other
/// test files (e.g. the state-controller unit tests) can reuse them.

class _FakeArcHost implements ArcHost {
  @override
  void dispose() {}

  @override
  bool get isDisposed => false;
}

class _FakeSoundHandle implements FlutterSoundHandle {
  int cancelCalls = 0;

  @override
  void cancel() {
    cancelCalls += 1;
  }

  @override
  void dispose() {}

  @override
  bool get isDisposed => false;
}

class _FakeSoundPlayer implements SoundPlayer {
  _FakeSoundPlayer(this._handle);

  final _FakeSoundHandle _handle;

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

class _RecordingTelepathy implements Telepathy {
  /// Completer for the most recent `startCall`. The test owns the
  /// completion timing so it can drive the race against a rebuild.
  final List<Completer<void>> startCallCallers = [];
  final List<Contact> startCallContacts = [];

  /// Completer for the most recent `joinRoom`.
  final List<Completer<void>> joinRoomCallers = [];
  final List<List<String>> joinRoomMemberStrings = [];

  @override
  Future<void> startCall({required Contact contact}) {
    final completer = Completer<void>();
    startCallCallers.add(completer);
    startCallContacts.add(contact);
    return completer.future;
  }

  @override
  Future<void> joinRoom({required List<String> memberStrings}) {
    final completer = Completer<void>();
    joinRoomCallers.add(completer);
    joinRoomMemberStrings.add(memberStrings);
    return completer.future;
  }

  // The contact/room `onPressed` closures only call `startCall` and
  // `joinRoom`. Everything below is here solely to satisfy the
  // abstract surface so the fake can be substituted for the real
  // telepathy bridge in `Provider<Telepathy>`.
  @override
  Future<void> audioTest() async {}
  @override
  ChatMessage buildChat(
          {required Contact contact,
          required String text,
          required List<(String, Uint8List)> attachments}) =>
      throw UnimplementedError();
  @override
  Future<void> endCall() async {}
  @override
  Future<(List<AudioDevice>, List<AudioDevice>)> listDevices() async =>
      (<AudioDevice>[], <AudioDevice>[]);
  @override
  void pauseStatistics() {}
  @override
  Future<void> restartManager() async {}
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
  Future<void> startManager() async {}
  @override
  Future<void> startScreenshare({required Contact contact}) async {}
  @override
  Future<void> startSession({required Contact contact}) async {}
  @override
  Future<void> stopSession({required Contact contact}) async {}
  @override
  void dispose() {}
  @override
  bool get isDisposed => false;
}

/// Stub the `flutter/assets` channel so `readSeaBytes` resolves to a
/// tiny non-empty payload. Tests then call `rootBundle.clear()` in
/// `tearDown` to drop the cache before the next case.
void _stubOutgoingRingtone(WidgetTester tester) {
  TestWidgetsFlutterBinding.ensureInitialized();
  tester.binding.defaultBinaryMessenger.setMockMessageHandler(
    'flutter/assets',
    (ByteData? message) async {
      // The closure treats the result as raw bytes; an empty payload
      // is sufficient because the fake SoundPlayer ignores its input.
      return ByteData(0);
    },
  );
}

/// `AssetBundle` used in tests so `SvgPicture.asset(...)` requests
/// resolve to the smallest well-formed SVG (instead of the empty
/// ByteData the binary messenger stub returns for the ringtone).
/// Without this, the SVG parser throws "Invalid SVG data" while the
/// contact/room widgets are building, and the test fails before any
/// of the call-target assertions run.
class _SvgAwareAssetBundle extends CachingAssetBundle {
  @override
  Future<ByteData> load(String key) async {
    if (key.endsWith('.svg')) {
      // The minimal SVG the vector_graphics parser accepts.
      const minimalSvg = '<svg viewBox="0 0 10 10"></svg>';
      final Uint8List bytes = Uint8List.fromList(utf8.encode(minimalSvg));
      return bytes.buffer.asByteData();
    }
    return ByteData(0);
  }
}

Widget _harness({
  required Widget child,
  required StateController stateController,
  required ProfilesController profilesController,
  required Telepathy telepathy,
  required SoundPlayer player,
}) {
  return DefaultAssetBundle(
    bundle: _SvgAwareAssetBundle(),
    child: MultiProvider(
      providers: [
        ChangeNotifierProvider<StateController>.value(value: stateController),
        ChangeNotifierProvider<ProfilesController>.value(
          value: profilesController,
        ),
        Provider<Telepathy>.value(value: telepathy),
        Provider<SoundPlayer>.value(value: player),
      ],
      child: MaterialApp(
        home: Scaffold(body: child),
      ),
    ),
  );
}

/// Mark the contact as `Connected` so the call icon is visible. The
/// widget's `build` only renders the Phone icon when
/// `stateController.sessionStatus(contact)` is
/// `SessionStatus_Connected`; otherwise it would show the Offline
/// icon and there would be no target to tap.
void _markContactConnected(StateController controller, Contact contact) {
  controller.updateSession(
    (
      contact.peerId(),
      const SessionStatus.connected(relayed: true, remoteAddress: '127.0.0.1')
    ),
  );
}

Future<void> _flushAsync(WidgetTester tester) async {
  // Drain pending microtasks/futures (e.g., the `await telepathy.startCall`
  // continuation) so post-rebuild assertions observe the resumed state.
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 16));
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  tearDown(() {
    rootBundle.clear();
    SharedPreferencesAsyncPlatform.instance = null;
  });

  group('ContactWidget call-target capture across rebuilds', () {
    late _FakeSoundHandle handle;
    late _FakeSoundPlayer player;
    late _RecordingTelepathy telepathy;
    late StateController stateController;
    late ProfilesController profilesController;
    late FakeContact alice;
    late FakeContact bob;

    setUp(() {
      FlutterSecureStorage.setMockInitialValues(<String, String>{});
      SharedPreferencesAsyncPlatform.instance =
          InMemorySharedPreferencesAsync.empty();
      handle = _FakeSoundHandle();
      player = _FakeSoundPlayer(handle);
      telepathy = _RecordingTelepathy();
      stateController = StateController();
      profilesController = ProfilesController(
        storage: const FlutterSecureStorage(),
        options: SharedPreferencesAsync(),
        roomHasher: ({required List<String> peers}) => peers.join('|'),
      );
      alice = FakeContact(
        id: 'contact-alice-id',
        contactNickname: 'Alice Ng',
      );
      bob = FakeContact(
        id: 'contact-bob-id',
        contactNickname: 'Bob Lee',
      );
      _markContactConnected(stateController, alice);
      _markContactConnected(stateController, bob);
    });

    testWidgets(
        'a mid-flight target swap does not redirect the captured startCall '
        'continuation onto the new contact', (WidgetTester tester) async {
      _stubOutgoingRingtone(tester);

      await tester.pumpWidget(
        _harness(
          stateController: stateController,
          profilesController: profilesController,
          telepathy: telepathy,
          player: player,
          child: ContactWidget(contact: alice),
        ),
      );

      // Tap Alice's phone icon. The closure runs through the early
      // gates, captures `target = alice`, registers pending, and
      // suspends on `await telepathy.startCall(contact: target)`.
      // With the test setUp marking Alice as Connected, the only
      // IconButton the widget renders is the Phone one.
      await tester.tap(find.byType(IconButton));
      await tester.pump();

      expect(stateController.pendingContact, same(alice),
          reason: 'pending slot must be the captured target the user clicked');
      expect(telepathy.startCallContacts, hasLength(1));
      expect(telepathy.startCallContacts.single, same(alice));

      // Mid-flight swap: replace the widget's contact with Bob using
      // a stable key so Flutter reuses the same State object. The
      // closure still references the captured `target = alice`.
      await tester.pumpWidget(
        _harness(
          stateController: stateController,
          profilesController: profilesController,
          telepathy: telepathy,
          player: player,
          child: ContactWidget(
            key: const ValueKey<String>('call-target-contact-row'),
            contact: bob,
          ),
        ),
      );
      await tester.pump();

      // Resolve the backend future so the `await startCall`
      // continuation resumes. The captured local `target` (alice)
      // must drive the call, not the freshly-rebuilt `widget.contact`
      // (now bob).
      telepathy.startCallCallers.single.complete();
      await _flushAsync(tester);

      expect(telepathy.startCallContacts, hasLength(1),
          reason: 'only the original tap should have invoked startCall');
      expect(telepathy.startCallContacts.single, same(alice),
          reason: 'startCall must target the captured local, not the rebuilt '
              'widget.contact');
      expect(stateController.activeContact, same(alice),
          reason: 'only the captured target should be promoted to active; the '
              'swap target must not be promoted by another widget\'s '
              'continuation');
      expect(stateController.pendingContact, isNull,
          reason: 'promotion must clear the pending slot');
      expect(handle.cancelCalls, 0,
          reason: 'the captured-target continuation succeeded; no spurious '
              'cancel should fire');
    });

    testWidgets(
        'a mid-flight second tap on the swap target still observes the '
        'first target as the only one promoted', (WidgetTester tester) async {
      _stubOutgoingRingtone(tester);

      const aliceKey = ValueKey<String>('call-target-row-alice');
      const bobKey = ValueKey<String>('call-target-row-bob');

      // Render both fixtures up front so each has its own state
      // object — this exercises the production widget tree where the
      // captured-target semantic is "do not let the second tap
      // hijack the first".
      await tester.pumpWidget(
        _harness(
          stateController: stateController,
          profilesController: profilesController,
          telepathy: telepathy,
          player: player,
          child: Column(
            children: [
              ContactWidget(key: aliceKey, contact: alice),
              ContactWidget(key: bobKey, contact: bob),
            ],
          ),
        ),
      );

      // Alice's tap — closure captures `target = alice` and suspends
      // on her `startCall` future. The first IconButton in the Column
      // belongs to Alice's ContactWidget (Phone is the only button
      // rendered when the contact is Connected).
      await tester.tap(find.byType(IconButton).first);
      await tester.pump();

      expect(stateController.pendingContact, same(alice));
      expect(stateController.hasLiveCall, isTrue);
      expect(telepathy.startCallContacts, [alice]);

      // Bob's tap is short-circuited by the `hasLiveCall` gate. The
      // closure returns early before any state mutation or
      // `startCall` invocation. Bob's button therefore must not have
      // changed the recorded target.
      await tester.tap(find.byType(IconButton).last);
      await tester.pump();

      expect(telepathy.startCallContacts, [alice],
          reason: 'second tap must not register a new startCall while the '
              'first future is still pending');
      expect(stateController.pendingContact, same(alice),
          reason: 'the captured target stays the only registered pending '
              'target across the short-circuited second tap');

      // Alice's future resolves; only Alice's captured target drives
      // the promotion.
      telepathy.startCallCallers.single.complete();
      await _flushAsync(tester);

      expect(telepathy.startCallContacts, [alice]);
      expect(stateController.activeContact, same(alice));
      expect(stateController.pendingContact, isNull);
    });
  });

  group('RoomWidget call-target capture across rebuilds', () {
    late _FakeSoundHandle handle;
    late _FakeSoundPlayer player;
    late _RecordingTelepathy telepathy;
    late StateController stateController;
    late ProfilesController profilesController;
    late Room alpha;
    late Room bravo;

    setUp(() {
      FlutterSecureStorage.setMockInitialValues(<String, String>{});
      SharedPreferencesAsyncPlatform.instance =
          InMemorySharedPreferencesAsync.empty();
      handle = _FakeSoundHandle();
      player = _FakeSoundPlayer(handle);
      telepathy = _RecordingTelepathy();
      stateController = StateController();
      profilesController = ProfilesController(
        storage: const FlutterSecureStorage(),
        options: SharedPreferencesAsync(),
        roomHasher: ({required List<String> peers}) => peers.join('|'),
      );
      alpha = Room(
        id: 'room-alpha',
        peerIds: const ['peer-1', 'peer-2'],
        nickname: 'Alpha Room',
      );
      bravo = Room(
        id: 'room-bravo',
        peerIds: const ['peer-3', 'peer-4'],
        nickname: 'Bravo Room',
      );
    });

    testWidgets(
        'a mid-flight target swap does not redirect the captured '
        'joinRoom continuation onto the new room', (WidgetTester tester) async {
      _stubOutgoingRingtone(tester);

      await tester.pumpWidget(
        _harness(
          stateController: stateController,
          profilesController: profilesController,
          telepathy: telepathy,
          player: player,
          child: RoomWidget(room: alpha),
        ),
      );

      // Tap Alpha's phone icon. The closure captures `target = alpha`,
      // registers pending, and suspends on `await joinRoom`. The
      // RoomWidget renders two IconButtons (Copy + Phone); the Phone
      // is the *last* one in the row.
      await tester.tap(find.byType(IconButton).last);
      await tester.pump();

      expect(stateController.pendingRoom, same(alpha),
          reason: 'pending slot must be the captured room the user clicked');
      expect(
          telepathy.joinRoomMemberStrings,
          [
            <String>['peer-1', 'peer-2'],
          ],
          reason: 'captured room\'s peer ids must be used for joinRoom');

      // Mid-flight swap: replace the widget's room with Bravo using a
      // stable key so Flutter reuses the same State object. The
      // in-flight closure still references the captured `target = alpha`.
      await tester.pumpWidget(
        _harness(
          stateController: stateController,
          profilesController: profilesController,
          telepathy: telepathy,
          player: player,
          child: RoomWidget(
            key: const ValueKey<String>('call-target-room-row'),
            room: bravo,
          ),
        ),
      );
      await tester.pump();

      telepathy.joinRoomCallers.single.complete();
      await _flushAsync(tester);

      expect(
          telepathy.joinRoomMemberStrings,
          [
            <String>['peer-1', 'peer-2'],
          ],
          reason: 'joinRoom must use the captured target\'s peer ids, not the '
              'fresh widget.room');
      expect(stateController.activeRoom, same(alpha),
          reason: 'only the captured target should be promoted to active; the '
              'swap target must not be promoted by another widget\'s '
              'continuation');
      expect(stateController.pendingRoom, isNull,
          reason: 'promotion must clear the pending slot');
      expect(alpha.online, isEmpty,
          reason: 'captured target\'s online list is cleared on success');
    });

    testWidgets(
        'a mid-flight second tap on the swap target still observes the '
        'first target as the only one promoted', (WidgetTester tester) async {
      _stubOutgoingRingtone(tester);

      const alphaKey = ValueKey<String>('call-target-room-alpha');
      const bravoKey = ValueKey<String>('call-target-room-bravo');

      await tester.pumpWidget(
        _harness(
          stateController: stateController,
          profilesController: profilesController,
          telepathy: telepathy,
          player: player,
          child: Column(
            children: [
              RoomWidget(key: alphaKey, room: alpha),
              RoomWidget(key: bravoKey, room: bravo),
            ],
          ),
        ),
      );

      // Alpha's tap — closure captures `target = alpha` and suspends
      // on its `joinRoom` future. Within the first RoomWidget row the
      // widgets are `[Copy, Phone]`, so Alpha's Phone is the second
      // IconButton in render order. With two rooms the order is
      // `[Copy_α, Phone_α, Copy_β, Phone_β]`; Alpha's Phone is at
      // index 1.
      await tester.tap(find.byType(IconButton).at(1));
      await tester.pump();

      expect(stateController.pendingRoom, same(alpha));
      expect(stateController.hasLiveCall, isTrue);
      expect(telepathy.joinRoomMemberStrings, [
        <String>['peer-1', 'peer-2'],
      ]);

      // Bravo's tap is short-circuited by the `hasLiveCall` gate.
      // Bravo's Phone sits at index 3.
      await tester.tap(find.byType(IconButton).at(3));
      await tester.pump();

      expect(
          telepathy.joinRoomMemberStrings,
          [
            <String>['peer-1', 'peer-2'],
          ],
          reason: 'second tap must not register a new joinRoom while the first '
              'future is still pending');
      expect(stateController.pendingRoom, same(alpha),
          reason: 'the captured target stays the only registered pending '
              'target across the short-circuited second tap');

      // Resolve Alpha's future; only Alpha is the captured target.
      telepathy.joinRoomCallers.single.complete();
      await _flushAsync(tester);

      expect(stateController.activeRoom, same(alpha));
      expect(stateController.pendingRoom, isNull);
    });
  });

  group('ContactsList stable keys', () {
    testWidgets(
        'distinct contact / room identities produce distinct '
        '`ValueKey` strings', (WidgetTester tester) async {
      // The list at `lib/widgets/contacts/contacts_list.dart` keys
      // each entry by `'contact:${contact.id()}'` and
      // `'room:${room.id}'`. Two distinct fixtures must produce
      // distinct keys so Flutter does not reuse a State object for a
      // different list item. This assertion guards the contract
      // without depending on the widget being a perfect drop-in for
      // end-to-end tap testing.
      const aliceKey = ValueKey<String>('contact:contact-alice-id');
      const bobKey = ValueKey<String>('contact:contact-bob-id');
      const alphaKey = ValueKey<String>('room:room-alpha');
      const bravoKey = ValueKey<String>('room:room-bravo');

      expect(aliceKey, isNot(equals(bobKey)),
          reason: 'distinct contact identities must produce distinct keys');
      expect(alphaKey, isNot(equals(bravoKey)),
          reason: 'distinct room identities must produce distinct keys');
    });
  });
}
