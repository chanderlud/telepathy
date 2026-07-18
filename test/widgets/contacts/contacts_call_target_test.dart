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
import 'package:telepathy/controllers/audio_settings_controller.dart';
import 'package:telepathy/controllers/profiles_controller.dart';
import 'package:telepathy/controllers/state_controller.dart';
import 'package:telepathy/core/rust/flutter.dart';
import 'package:telepathy/core/rust/lib.dart';
import 'package:telepathy/core/rust/player.dart';
import 'package:telepathy/core/rust/types.dart';
import 'package:telepathy/core/utils/console.dart';
import 'package:telepathy/core/utils/sound_effects.dart';
import 'package:telepathy/models/index.dart';
import 'package:telepathy/widgets/call/call_controls.dart';
import 'package:telepathy/widgets/call/room_details_widget.dart';
import 'package:telepathy/widgets/contacts/contact_widget.dart';
import 'package:telepathy/widgets/contacts/room_widget.dart';

import '../../support/fake_contact.dart';

/// Fakes of the rust bridge types used by the contact/room widgets.
///
/// `Contact`, `Telepathy`, `SoundPlayer`, `FlutterSoundHandle`, `PublicKey`, and
/// `ArcHost` are `RustOpaqueInterface` markers — they only require
/// `dispose()`/`isDisposed` at runtime, the rest are abstract. The widget test
/// exercises only the contact/room call icon's `onPressed` closure, so the fakes
/// return no-op values for surfaces the closure does not touch. `Contact` and
/// `PublicKey` live in `test/support/fake_contact.dart` for reuse.

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

class _ThrowingSoundPlayer implements SoundPlayer {
  _ThrowingSoundPlayer(this.errorMessage);

  final String errorMessage;
  int playCalls = 0;

  @override
  ArcHost host() => _FakeArcHost();

  @override
  Future<FlutterSoundHandle> play({required List<int> bytes}) async {
    playCalls += 1;
    throw DartError(message: errorMessage);
  }

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
  int endCallCalls = 0;
  Completer<void>? endCallCompleter;
  final List<bool> mutedValues = [];
  final List<bool> deafenedValues = [];

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

  // Remaining members satisfy the abstract bridge surface.
  @override
  Future<void> audioTest() async {}
  @override
  ChatMessage buildChat(
          {required Contact contact,
          required String text,
          required List<(String, Uint8List)> attachments}) =>
      throw UnimplementedError();
  @override
  Future<void> endCall() {
    endCallCalls += 1;
    return endCallCompleter?.future ?? Future<void>.value();
  }

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
  void setDeafened({required bool deafened}) {
    deafenedValues.add(deafened);
  }

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
  void setMuted({required bool muted}) {
    mutedValues.add(muted);
  }

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

/// Stub the `flutter/assets` channel so `readSeaBytes` resolves to an empty
/// payload. The fake SoundPlayer ignores its input, and `rootBundle.clear()` in
/// `tearDown` drops the cache before the next case.
void _stubOutgoingRingtone(WidgetTester tester) {
  TestWidgetsFlutterBinding.ensureInitialized();
  tester.binding.defaultBinaryMessenger.setMockMessageHandler(
    'flutter/assets',
    (ByteData? message) async {
      return ByteData(0);
    },
  );
}

/// AssetBundle that resolves `.svg` requests to the minimal well-formed SVG; the
/// binary messenger stub returns empty ByteData for the ringtone. Without this
/// fallback the SVG parser throws "Invalid SVG data" while the widgets build.
class _SvgAwareAssetBundle extends CachingAssetBundle {
  @override
  Future<ByteData> load(String key) async {
    if (key.endsWith('.svg')) {
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

/// Mark the contact as `Connected` so the call icon renders; otherwise the
/// widget shows the Offline icon and there is no target to tap.
void _markContactConnected(StateController controller, Contact contact) {
  controller.updateSession(
    (
      contact.peerId(),
      const SessionStatus.connected(relayed: true, remoteAddress: '127.0.0.1')
    ),
  );
}

Future<void> _flushAsync(WidgetTester tester) async {
  // Drain pending microtasks/futures so post-rebuild assertions observe the
  // resumed state.
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 16));
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  tearDown(() {
    outgoingSoundHandle?.cancel();
    otherSoundHandle?.cancel();
    outgoingSoundHandle = null;
    otherSoundHandle = null;
    rootBundle.clear();
    SharedPreferencesAsyncPlatform.instance = null;
  });

  testWidgets(
      'playSoundEffect returns null and logs DartError context when sound playback fails',
      (WidgetTester _) async {
    final _ThrowingSoundPlayer player = _ThrowingSoundPlayer(
      'SoundPlayer.play failed for outgoing ringtone while output device is locked',
    );
    final int originalLogCount = console.logs.length;

    addTearDown(() {
      console.logs.removeRange(originalLogCount, console.logs.length);
    });

    final FlutterSoundHandle? handle = await playSoundEffect(
      player: player,
      bytes: <int>[1, 2, 3],
      sound: 'outgoing ringtone',
    );

    expect(handle, isNull,
        reason: 'DartError playback failures must surface as null handles');
    expect(player.playCalls, 1,
        reason: 'helper must still attempt playback once');
    expect(console.logs, hasLength(originalLogCount + 1),
        reason: 'failure path must emit exactly one console error');

    final Log log = console.logs.last;
    expect(log.type, 'error');
    expect(log.message,
        'Failed to play outgoing ringtone sound: SoundPlayer.play failed for outgoing ringtone while output device is locked');
    expect(log.message, contains('outgoing ringtone'));
    expect(
        log.message,
        contains(
            'SoundPlayer.play failed for outgoing ringtone while output device is locked'));
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
        'a locked output device does not stop the call target from reaching '
        'startCall', (WidgetTester tester) async {
      _stubOutgoingRingtone(tester);

      final _ThrowingSoundPlayer throwingPlayer = _ThrowingSoundPlayer(
        'SoundPlayer.play failed for outgoing ringtone while output device is locked',
      );

      await tester.pumpWidget(
        _harness(
          stateController: stateController,
          profilesController: profilesController,
          telepathy: telepathy,
          player: throwingPlayer,
          child: ContactWidget(contact: alice),
        ),
      );

      await tester.tap(find.byType(IconButton));
      await tester.pump();

      expect(stateController.pendingContact, same(alice),
          reason: 'the clicked contact still becomes pending before sound '
              'playback fails');
      expect(telepathy.startCallContacts, hasLength(1),
          reason: 'backend request must start before optional sound playback');
      telepathy.startCallCallers.single.complete();
      await _flushAsync(tester);
      expect(throwingPlayer.playCalls, 1,
          reason: 'best-effort outgoing sound must still attempt playback '
              'after the backend accepts the call');
      expect(telepathy.startCallContacts, hasLength(1),
          reason: 'best-effort outgoing sound must not block startCall even '
              'when SoundPlayer.play throws DartError');
      expect(telepathy.startCallContacts.single, same(alice),
          reason: 'startCall must still target the clicked contact');
    });

    testWidgets('a never-connected direct call can be cancelled',
        (WidgetTester tester) async {
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

      await tester.tap(find.byType(IconButton));
      await tester.pump();

      expect(stateController.pendingContact, same(alice));
      expect(find.bySemanticsLabel('End call icon'), findsOneWidget);
      expect(find.bySemanticsLabel('Call icon'), findsNothing);

      await tester.tap(find.bySemanticsLabel('End call icon'));
      await tester.pump();

      expect(telepathy.endCallCalls, 1);
      expect(stateController.callLifecycle, CallLifecycle.idle);
      expect(stateController.pendingContact, isNull);
      await tester.pump(const Duration(seconds: 1));
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

      // Tap Alice's phone icon. The closure captures `target = alice`, registers
      // pending, and suspends on `await telepathy.startCall(contact: target)`.
      await tester.tap(find.byType(IconButton));
      await tester.pump();

      expect(stateController.pendingContact, same(alice),
          reason: 'pending slot must be the captured target the user clicked');
      expect(telepathy.startCallContacts, hasLength(1));
      expect(telepathy.startCallContacts.single, same(alice));

      // Mid-flight swap: stable key so Flutter reuses the same State object;
      // the closure still references the captured `target = alice`.
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

      // Connected may arrive before the start future resolves. It owns
      // promotion; the continuation must not change the active target again.
      stateController
          .promotePendingCallAttempt(stateController.currentCallAttempt);
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

      // Render both fixtures so each has its own State object — exercises the
      // production tree where captured-target semantics prevent a second tap
      // from hijacking the first.
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

      await tester.tap(find.byType(IconButton).first);
      await tester.pump();

      expect(stateController.pendingContact, same(alice));
      expect(stateController.hasLiveCall, isTrue);
      expect(telepathy.startCallContacts, [alice]);

      await tester.tap(find.byType(IconButton).last);
      await tester.pump();

      expect(telepathy.startCallContacts, [alice],
          reason: 'second tap must not register a new startCall while the '
              'first future is still pending');
      expect(stateController.pendingContact, same(alice),
          reason: 'the captured target stays the only registered pending '
              'target across the short-circuited second tap');

      stateController
          .promotePendingCallAttempt(stateController.currentCallAttempt);
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
        'a locked output device does not stop a room call from reaching '
        'joinRoom', (WidgetTester tester) async {
      _stubOutgoingRingtone(tester);

      final _ThrowingSoundPlayer throwingPlayer = _ThrowingSoundPlayer(
        'SoundPlayer.play failed for outgoing ringtone while output device is locked',
      );

      await tester.pumpWidget(
        _harness(
          stateController: stateController,
          profilesController: profilesController,
          telepathy: telepathy,
          player: throwingPlayer,
          child: RoomWidget(room: alpha),
        ),
      );

      await tester.tap(find.byType(IconButton).last);
      await tester.pump();

      expect(stateController.pendingRoom, same(alpha),
          reason:
              'the selected room must become pending before playback fails');
      expect(telepathy.joinRoomMemberStrings, [alpha.peerIds],
          reason: 'backend request must start before optional sound playback');
      telepathy.joinRoomCallers.single.complete();
      await _flushAsync(tester);
      expect(throwingPlayer.playCalls, 1,
          reason: 'best-effort outgoing sound must attempt playback after '
              'the backend accepts the room request');
      expect(telepathy.joinRoomMemberStrings, [alpha.peerIds],
          reason: 'best-effort outgoing sound must not block joinRoom when '
              'SoundPlayer.play throws DartError');
    });

    testWidgets('an empty room can be cancelled before any peer connects',
        (WidgetTester tester) async {
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

      await tester.tap(find.byType(IconButton).last);
      await tester.pump();

      expect(stateController.pendingRoom, same(alpha));
      expect(alpha.online, isEmpty);
      expect(find.bySemanticsLabel('End call icon'), findsOneWidget);
      expect(find.bySemanticsLabel('Call icon'), findsNothing);

      await tester.tap(find.bySemanticsLabel('End call icon'));
      await tester.pump();

      expect(telepathy.endCallCalls, 1);
      expect(stateController.callLifecycle, CallLifecycle.idle);
      expect(stateController.pendingRoom, isNull);
      await tester.pump(const Duration(seconds: 1));
    });

    testWidgets(
        'a synchronous room setup error resets state and shows one dialog',
        (WidgetTester tester) async {
      _stubOutgoingRingtone(tester);
      var stateResets = 0;
      var previousLifecycle = stateController.callLifecycle;
      stateController.addListener(() {
        if (previousLifecycle != CallLifecycle.idle &&
            stateController.callLifecycle == CallLifecycle.idle) {
          stateResets += 1;
        }
        previousLifecycle = stateController.callLifecycle;
      });

      await tester.pumpWidget(
        _harness(
          stateController: stateController,
          profilesController: profilesController,
          telepathy: telepathy,
          player: player,
          child: RoomWidget(room: alpha),
        ),
      );

      await tester.tap(find.byType(IconButton).last);
      await tester.pump();
      telepathy.joinRoomCallers.single.completeError(
        const DartError(message: 'selected input device is unavailable'),
      );
      await tester.pumpAndSettle();

      expect(stateResets, 1,
          reason: 'returned setup error must be the only reset path');
      expect(stateController.callLifecycle, CallLifecycle.idle);
      expect(find.text('Call failed'), findsOneWidget);
      expect(find.text('selected input device is unavailable'), findsOneWidget);
      await tester.pump(const Duration(seconds: 1));
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

      // Tap Alpha's phone icon. RoomWidget renders [Copy, Phone] in row order;
// Phone is the last IconButton.
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

      // Mid-flight swap: stable key so Flutter reuses the same State object;
      // the in-flight closure still references the captured `target = alpha`.
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

      stateController
          .promotePendingCallAttempt(stateController.currentCallAttempt);
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

      // Each row's IconButtons are [Copy, Phone]; with two rooms the order is
      // [Copy_α, Phone_α, Copy_β, Phone_β]. Alpha's Phone is at index 1.
      await tester.tap(find.byType(IconButton).at(1));
      await tester.pump();

      expect(stateController.pendingRoom, same(alpha));
      expect(stateController.hasLiveCall, isTrue);
      expect(telepathy.joinRoomMemberStrings, [
        <String>['peer-1', 'peer-2'],
      ]);

      // Bravo's Phone is at index 3.
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

      stateController
          .promotePendingCallAttempt(stateController.currentCallAttempt);
      telepathy.joinRoomCallers.single.complete();
      await _flushAsync(tester);

      expect(stateController.activeRoom, same(alpha));
      expect(stateController.pendingRoom, isNull);
    });
  });

  group('Optional call sounds do not block controls', () {
    late _RecordingTelepathy telepathy;
    late StateController stateController;
    late ProfilesController profilesController;
    late Room alpha;

    setUp(() {
      FlutterSecureStorage.setMockInitialValues(<String, String>{});
      SharedPreferencesAsyncPlatform.instance =
          InMemorySharedPreferencesAsync.empty();
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
    });

    testWidgets('a locked output device does not stop room end cleanup',
        (WidgetTester tester) async {
      _stubOutgoingRingtone(tester);
      profilesController.profiles['profile-test'] = Profile(
        id: 'profile-test',
        nickname: 'Test Profile',
        peerId: '12D3KooWTestProfilePeerId111111111111111111111111111111',
        keypair: const <int>[],
        contacts: <String, Contact>{},
        rooms: <String, Room>{},
      );
      profilesController.activeProfile = 'profile-test';
      stateController.setActiveRoom(alpha);
      final _ThrowingSoundPlayer throwingPlayer = _ThrowingSoundPlayer(
        'SoundPlayer.play failed for call ended sound while output device is locked',
      );

      await tester.pumpWidget(
        _harness(
          stateController: stateController,
          profilesController: profilesController,
          telepathy: telepathy,
          player: throwingPlayer,
          child: const RoomDetailsWidget(),
        ),
      );

      await tester.tap(find.byType(IconButton));
      await tester.pump();

      expect(throwingPlayer.playCalls, 1,
          reason: 'best-effort end sound must attempt playback once');
      expect(telepathy.endCallCalls, 1,
          reason: 'end sound failure must not block backend endCall');
      expect(stateController.activeRoom, isNull,
          reason: 'end sound failure must not retain active room state');
      expect(stateController.status, 'Inactive',
          reason: 'end sound failure must reset call status');
      await tester.pump(const Duration(seconds: 1));
    });

    testWidgets('hangup keeps teardown gate closed until backend confirms',
        (WidgetTester tester) async {
      _stubOutgoingRingtone(tester);
      profilesController.profiles['profile-test'] = Profile(
        id: 'profile-test',
        nickname: 'Test Profile',
        peerId: '12D3KooWTestProfilePeerId111111111111111111111111111111',
        keypair: const <int>[],
        contacts: <String, Contact>{},
        rooms: <String, Room>{},
      );
      profilesController.activeProfile = 'profile-test';
      stateController.setActiveRoom(alpha);
      telepathy.endCallCompleter = Completer<void>();

      await tester.pumpWidget(
        _harness(
          stateController: stateController,
          profilesController: profilesController,
          telepathy: telepathy,
          player: _FakeSoundPlayer(_FakeSoundHandle()),
          child: const RoomDetailsWidget(),
        ),
      );

      await tester.tap(find.byType(IconButton));
      await tester.pump();

      expect(stateController.callLifecycle, CallLifecycle.ending);
      expect(stateController.blockAudioChanges, isTrue,
          reason: 'settings must remain blocked while endCall is pending');

      telepathy.endCallCompleter!.complete();
      await _flushAsync(tester);

      expect(stateController.callLifecycle, CallLifecycle.idle);
      expect(stateController.blockAudioChanges, isFalse);
      await tester.pump(const Duration(seconds: 1));
    });

    testWidgets('a locked output device does not stop mute state or backend',
        (WidgetTester tester) async {
      _stubOutgoingRingtone(tester);
      final audioSettings = AudioSettingsController(
        options: SharedPreferencesAsync(),
      );
      await audioSettings.init();
      final _ThrowingSoundPlayer throwingPlayer = _ThrowingSoundPlayer(
        'SoundPlayer.play failed for mute sound while output device is locked',
      );

      await tester.pumpWidget(
        ChangeNotifierProvider<AudioSettingsController>.value(
          value: audioSettings,
          child: _harness(
            stateController: stateController,
            profilesController: profilesController,
            telepathy: telepathy,
            player: throwingPlayer,
            child: const CallControls(),
          ),
        ),
      );

      await tester.tap(find.byType(IconButton).first);
      await tester.pump();

      expect(throwingPlayer.playCalls, 1,
          reason: 'best-effort mute sound must attempt playback once');
      expect(stateController.isMuted, isTrue,
          reason: 'mute sound failure must not block muted state');
      expect(telepathy.mutedValues, [true],
          reason: 'mute sound failure must not block setMuted');
    });

    testWidgets('a locked output device does not stop deafen state or backend',
        (WidgetTester tester) async {
      _stubOutgoingRingtone(tester);
      final audioSettings = AudioSettingsController(
        options: SharedPreferencesAsync(),
      );
      await audioSettings.init();
      final _ThrowingSoundPlayer throwingPlayer = _ThrowingSoundPlayer(
        'SoundPlayer.play failed for deafen sound while output device is locked',
      );

      await tester.pumpWidget(
        ChangeNotifierProvider<AudioSettingsController>.value(
          value: audioSettings,
          child: _harness(
            stateController: stateController,
            profilesController: profilesController,
            telepathy: telepathy,
            player: throwingPlayer,
            child: const CallControls(),
          ),
        ),
      );

      await tester.tap(find.byType(IconButton).at(1));
      await tester.pump();

      expect(throwingPlayer.playCalls, 1,
          reason: 'best-effort deafen sound must attempt playback once');
      expect(stateController.isDeafened, isTrue,
          reason: 'deafen sound failure must not block deafened state');
      expect(stateController.isMuted, isTrue,
          reason: 'deafen sound failure must retain enforced mute state');
      expect(telepathy.deafenedValues, [true],
          reason: 'deafen sound failure must not block setDeafened');
      expect(telepathy.mutedValues, [true],
          reason: 'deafen sound failure must not block setMuted');
    });
  });

  group('ContactsList stable keys', () {
    testWidgets(
        'distinct contact / room identities produce distinct '
        '`ValueKey` strings', (WidgetTester tester) async {
      // `lib/widgets/contacts/contacts_list.dart` keys entries by
      // `'contact:${contact.id()}'` and `'room:${room.id}'`. Distinct
      // fixtures must produce distinct keys so Flutter does not reuse
      // a State object for a different list item.
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
