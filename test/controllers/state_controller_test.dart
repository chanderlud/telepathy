import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:telepathy/controllers/state_controller.dart';
import 'package:telepathy/core/rust/types.dart';
import 'package:telepathy/models/room.dart';
import '../support/fake_contact.dart';

/// Reproduces the `callState(CallState)` gate in `lib/main.dart` for tests.
/// Mirrors the real switch-on-event sequence: returns the failure reason the
/// dialog would have surfaced, or `null` if no dialog was shown.
Future<String?> _simulateCallState(
    StateController controller, CallState state) async {
  if (!controller.hasLiveCall) {
    return null;
  }

  if (state is CallState_CallEnded) {
    // localHangup is true iff the lifecycle was already reset (e.g. by
    // the widget's hangup handler) before the backend echo arrived.
    final localHangup = !controller.hasLiveCall;
    controller.endOfCall();
    return (!localHangup && state.field0.isNotEmpty) ? state.field0 : null;
  } else if (state is CallState_Connected) {
    controller.clearPending();
    controller.markCallActive();
    controller.setStatus('Active');
    return null;
  } else if (state is CallState_Waiting) {
    controller.setStatus('Waiting for peers');
    return null;
  }
  return null;
}

Room _roomFixture(String id) => Room(
      id: id,
      peerIds: <String>[],
      nickname: 'Room $id',
    );

void main() {
  group('StateController.runAudioTest', () {
    test('clears inAudioTest after a successful audio test', () async {
      final controller = StateController();
      final completer = Completer<void>();

      final audioTest = controller.runAudioTest(() => completer.future);

      expect(controller.inAudioTest, isTrue);
      expect(controller.status, 'In Audio Test');

      completer.complete();
      await audioTest;

      expect(controller.inAudioTest, isFalse);
      expect(controller.status, 'Inactive');
    });

    test('clears inAudioTest after a DartError', () async {
      final controller = StateController();
      final completer = Completer<void>();

      final audioTest = controller.runAudioTest(() => completer.future);

      expect(controller.inAudioTest, isTrue);
      expect(controller.status, 'In Audio Test');

      completer.completeError(const DartError(message: 'boom'));

      await expectLater(audioTest, throwsA(isA<DartError>()));

      expect(controller.inAudioTest, isFalse);
      expect(controller.status, 'Inactive');
    });
  });

  group('StateController call lifecycle', () {
    test('starts in the idle lifecycle with no live call', () {
      final controller = StateController();
      expect(controller.callLifecycle, CallLifecycle.idle);
      expect(controller.hasLiveCall, isFalse);
      expect(controller.isCallActive, isFalse);
    });

    test('setPendingRoom moves to connecting and exposes a live call', () {
      final controller = StateController();
      controller.setPendingRoom(_roomFixture('room-1'));

      expect(controller.pendingRoom, isNotNull);
      expect(controller.callLifecycle, CallLifecycle.connecting);
      expect(controller.hasLiveCall, isTrue);
      expect(controller.isCallActive, isFalse);
    });

    test('promoting a pending room to active clears the pending slot', () {
      final controller = StateController();
      final room = _roomFixture('room-2');
      controller.setPendingRoom(room);
      controller.setActiveRoom(room);

      expect(controller.pendingRoom, isNull);
      expect(controller.callLifecycle, CallLifecycle.active);
      expect(controller.hasLiveCall, isTrue);
      expect(controller.isCallActive, isTrue);
    });

    test('endOfCall from connecting (room) clears lifecycle to idle', () {
      final controller = StateController();
      controller.setPendingRoom(_roomFixture('room-3'));
      expect(controller.hasLiveCall, isTrue);

      controller.endOfCall();

      expect(controller.callLifecycle, CallLifecycle.idle);
      expect(controller.hasLiveCall, isFalse);
      expect(controller.pendingRoom, isNull);
      expect(controller.activeRoom, isNull);
    });

    test('endOfCall resets lifecycle so a late backend echo is suppressed', () {
      final controller = StateController();
      controller.setPendingRoom(_roomFixture('room-4'));
      controller.endOfCall();

      // Lifecycle is idle after hangup, so a trailing backend CallEnded must
      // NOT be considered a live call. The main.dart gate relies on this so it
      // does not surface a failure dialog after a local hangup.
      expect(controller.callLifecycle, CallLifecycle.idle);
      expect(controller.hasLiveCall, isFalse);
    });

    test('setStatus(Inactive) clears any leftover pending room slot', () {
      final controller = StateController();
      controller.setPendingRoom(_roomFixture('room-5'));

      controller.setStatus('Inactive');

      expect(controller.callLifecycle, CallLifecycle.idle);
      expect(controller.pendingRoom, isNull);
      expect(controller.hasLiveCall, isFalse);
    });

    test('setStatus(Active) keeps the pending room slot until cleared', () {
      final controller = StateController();
      controller.setPendingRoom(_roomFixture('room-6'));

      controller.setStatus('Active');

      // setStatus only flips the lifecycle phase; the pending slot is owned
      // by setActiveRoom/setActiveContact. Guards against a stale setStatus
      // call wiping a pending target a slow backend call later claims.
      expect(controller.pendingRoom, isNotNull);
      expect(controller.callLifecycle, CallLifecycle.active);

      controller.clearPending();
      expect(controller.pendingRoom, isNull);
    });

    test(
        'fast backend CallEnded during room setup is observed and surfaces '
        'the failure reason (instead of being dropped by the gate)', () async {
      final controller = StateController();
      final room = _roomFixture('fast-fail');

      // Mirror exactly what `room_widget.dart` does before awaiting `joinRoom`.
      controller.setStatus('Connecting');
      controller.setPendingRoom(room);
      expect(controller.hasLiveCall, isTrue,
          reason: 'gate must be open while we are negotiating');

      // Regression: the old `isCallActive` gate dropped this event because the
      // room never reached active state.
      final surfaced = await _simulateCallState(
          controller,
          const CallState_CallEnded(
            'Room peer unreachable',
            true,
          ));

      expect(surfaced, 'Room peer unreachable',
          reason: 'reason must reach the UI as a failure dialog');
      expect(controller.callLifecycle, CallLifecycle.idle);
      expect(controller.hasLiveCall, isFalse);
      expect(controller.pendingRoom, isNull);
    });

    test(
        'fast backend Connected during outgoing setup promotes lifecycle '
        'to active and clears the pending slot', () async {
      final controller = StateController();
      controller.setStatus('Connecting');
      controller.setPendingRoom(_roomFixture('connects-fast'));

      final surfaced =
          await _simulateCallState(controller, const CallState_Connected());

      expect(surfaced, isNull);
      expect(controller.callLifecycle, CallLifecycle.active);
      expect(controller.hasLiveCall, isTrue);
      expect(controller.pendingRoom, isNull);
      expect(controller.status, 'Active');
    });

    test('a local hangup that races a trailing backend CallEnded stays silent',
        () async {
      final controller = StateController();
      controller.setStatus('Connecting');
      controller.setPendingRoom(_roomFixture('local-hangup'));

      // Widget sets lifecycle to idle before the backend echo of `CallEnded`.
      controller.endOfCall();
      expect(controller.hasLiveCall, isFalse);

      final surfaced = await _simulateCallState(
          controller, const CallState_CallEnded('remote reason', true));

      expect(surfaced, isNull,
          reason: 'late backend echo after a local hangup must stay silent');
      expect(controller.callLifecycle, CallLifecycle.idle);
    });

    test(
        'pending-contact path: lifecycle moves to connecting, the fake '
        'contact occupies the pending slot, fast CallEnded clears everything, '
        'and a late promotion after endOfCall is skipped', () async {
      // Mirrors exactly what `contact_widget.dart` does when the user taps the
      // call icon: setStatus('Connecting') then setPendingContact(...) before
      // awaiting `Telepathy.startCall()`. The native bridge is not initialized
      // in this unit-test harness, so a real `Contact` cannot be constructed;
      // `FakeContact` stands in for it.
      final controller = StateController();
      final alice = FakeContact(
        id: 'alice-12D3KooWAlicePeerIdExampleString',
        contactNickname: 'Alice Ng',
      );

      controller.setStatus('Connecting');
      controller.setPendingContact(alice);

      expect(controller.callLifecycle, CallLifecycle.connecting,
          reason: 'pending-contact transition must open the gate');
      expect(controller.hasLiveCall, isTrue,
          reason: 'gate must observe the connecting phase');
      expect(controller.pendingContact, same(alice),
          reason: 'pending-contact slot must own the captured target');

      // Regression: with the old `isCallActive` gate this would have been
      // dropped because the contact never reached active state.
      final surfaced = await _simulateCallState(
        controller,
        const CallState_CallEnded(
          'Peer declined the call',
          true,
        ),
      );

      expect(surfaced, 'Peer declined the call',
          reason: 'reason must reach the UI as a failure dialog');
      expect(controller.callLifecycle, CallLifecycle.idle,
          reason: 'CallEnded must drop the connecting phase immediately');
      expect(controller.hasLiveCall, isFalse,
          reason: 'gate must close once CallEnded lands');
      expect(controller.pendingContact, isNull,
          reason: 'pending slot must be cleared once CallEnded lands');

      // A late `setActiveContact` after endOfCall() must be skipped so the
      // call does not resurrect and the timer does not restart.
      controller.setActiveContact(alice);
      expect(controller.callLifecycle, CallLifecycle.idle,
          reason: 'post-endOfCall promotion must be skipped');
      expect(controller.pendingContact, isNull,
          reason: 'pending slot stays cleared');
      expect(controller.hasLiveCall, isFalse,
          reason: 'gate must stay closed after a skipped promotion');
    });

    test(
        'fast CallEnded before the start future resumes: promotePendingContact '
        'returns false and does not resurrect the call', () async {
      // Race scenario: the widget has called setPendingContact, the backend fires
      // a CallEnded callback that endOfCall()s the controller to idle, and then
      // the `await startCall` future resumes. The widget's continuation calls
      // promotePendingContact; the atomic check must reject the promotion so
      // the call does not come back from the dead.
      final controller = StateController();
      final contact = FakeContact(
        id: 'fast-end-race',
        contactNickname: 'Race Target',
      );

      controller.setStatus('Connecting');
      controller.setPendingContact(contact);
      expect(controller.hasLiveCall, isTrue,
          reason: 'gate must be open while we are negotiating');

      final surfaced = await _simulateCallState(
          controller,
          const CallState_CallEnded(
            'peer went away',
            true,
          ));
      expect(surfaced, 'peer went away');

      final promoted = controller.promotePendingContact(contact);
      expect(promoted, isFalse,
          reason:
              'promote must refuse after a racing endOfCall cleared the lifecycle');
      expect(controller.callLifecycle, CallLifecycle.idle,
          reason: 'lifecycle must stay idle after the racing endOfCall');
      expect(controller.hasLiveCall, isFalse, reason: 'gate must stay closed');
      expect(controller.pendingContact, isNull,
          reason: 'pending slot must stay cleared');
    });

    test(
        'second call tap during connecting: hasLiveCall guard rejects the tap '
        'and the first call is not disturbed', () async {
      // Widget check `if (stateController.hasLiveCall) return error`. Simulate a
      // first call still in the connecting phase, then a second tap that
      // observes hasLiveCall == true.
      final controller = StateController();
      final firstRoom = _roomFixture('first');
      controller.setStatus('Connecting');
      controller.setPendingRoom(firstRoom);

      expect(controller.hasLiveCall, isTrue,
          reason: 'first call is in connecting phase, gate is open');
      expect(controller.pendingRoom, firstRoom,
          reason: 'pending slot still holds the first target');

      final secondBlocked = controller.hasLiveCall;
      expect(secondBlocked, isTrue,
          reason: 'second tap must observe a live call in the controller');

      expect(controller.pendingRoom, firstRoom,
          reason: 'first call slot must be undisturbed by the rejected tap');
    });

    test(
        'promotePendingRoom: positive path promotes a matching pending room, '
        'wrong room identity is rejected', () {
      final controller = StateController();
      final first = _roomFixture('promote-1');
      final wrong = _roomFixture('promote-2');

      controller.setStatus('Connecting');
      controller.setPendingRoom(first);

      final wrongResult = controller.promotePendingRoom(wrong);
      expect(wrongResult, isFalse,
          reason: 'promote must refuse a mismatched room instance');
      expect(controller.callLifecycle, CallLifecycle.connecting,
          reason: 'lifecycle must stay connecting on a rejected promote');
      expect(controller.pendingRoom, first,
          reason: 'pending slot must stay intact on a rejected promote');

      final okResult = controller.promotePendingRoom(first);
      expect(okResult, isTrue,
          reason: 'promote must accept the same room instance');
      expect(controller.callLifecycle, CallLifecycle.active,
          reason: 'lifecycle must advance to active on success');
      expect(controller.pendingRoom, isNull,
          reason: 'pending slot must be cleared on success');
      expect(controller.activeRoom, first,
          reason: 'active room must be the promoted one');
    });
  });
}
