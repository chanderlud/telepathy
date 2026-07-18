import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:telepathy/controllers/state_controller.dart';
import 'package:telepathy/core/rust/types.dart';
import 'package:telepathy/models/room.dart';
import '../support/fake_contact.dart';

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

    test('promoting a pending room attempt clears the pending slot', () {
      final controller = StateController();
      final room = _roomFixture('room-2');
      final attempt = controller.setPendingRoom(room);
      controller.promotePendingCallAttempt(attempt);

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

    test(
        'fast backend CallEnded during room setup is observed by the gate '
        'and clears the lifecycle to idle', () {
      final controller = StateController();
      final room = _roomFixture('fast-fail');

      // Mirror exactly what `room_widget.dart` does before awaiting `joinRoom`.
      controller.setStatus('Connecting');
      controller.setPendingRoom(room);
      expect(controller.hasLiveCall, isTrue,
          reason: 'gate must be open while we are negotiating');

      // Production handler invoked by `main.dart`'s `callState()` switch once
      // the gate observes the `CallEnded` event. Dialog surfacing is owned by
      // `main.dart`, so the controller-level contract is only that the
      // lifecycle clears to idle while the gate was open.
      controller.endOfCall();

      expect(controller.callLifecycle, CallLifecycle.idle);
      expect(controller.hasLiveCall, isFalse);
      expect(controller.pendingRoom, isNull);
    });

    test(
        'fast backend Connected during outgoing setup promotes lifecycle '
        'to active and clears the pending slot', () {
      final controller = StateController();
      controller.setStatus('Connecting');
      final room = _roomFixture('connects-fast');
      controller.setPendingRoom(room);

      // Production handler invoked by `main.dart`'s `callState()` switch.
      final accepted =
          controller.handleConnectedEvent(controller.currentCallAttempt);

      expect(accepted, isTrue,
          reason: 'first Connected must promote the connecting lifecycle');
      expect(controller.callLifecycle, CallLifecycle.active);
      expect(controller.hasLiveCall, isTrue);
      expect(controller.pendingRoom, isNull);
      expect(controller.status, 'Active');
      expect(controller.activeRoom, same(room));
    });

    test(
        'room Connected after Waiting resets stale status and runs the '
        'connected path', () {
      // Regression: previously a `Connected` event arriving after `Waiting`
      // was discarded because `promotePendingCallAttempt` returned false on
      // the already-active lifecycle. The stale `Waiting for peers` status
      // lingered and the connected sound was suppressed.
      final controller = StateController();
      controller.setStatus('Connecting');
      final room = _roomFixture('waiting-then-connected');
      controller.setPendingRoom(room);

      // `main.dart`'s `callState()` promotes the room on `Waiting` so the
      // call controls render, then surfaces the waiting status. Both calls
      // are production handlers exercised verbatim here.
      controller.promotePendingCallAttempt(controller.currentCallAttempt);
      controller.setStatus('Waiting for peers');

      expect(controller.callLifecycle, CallLifecycle.active);
      expect(controller.status, 'Waiting for peers');
      expect(controller.activeRoom, same(room));

      // Production handler invoked when the `Connected` event arrives. Must
      // NOT be rejected just because the room is already active after the
      // earlier `Waiting` promotion.
      final accepted =
          controller.handleConnectedEvent(controller.currentCallAttempt);

      expect(accepted, isTrue,
          reason:
              'connected path must run for an already-active room after Waiting');
      expect(controller.callLifecycle, CallLifecycle.active);
      expect(controller.status, 'Active',
          reason: 'stale Waiting-for-peers label must be cleared');
      expect(controller.activeRoom, same(room));
    });

    test(
        'Connected is rejected in idle and ending lifetimes even for the '
        'current attempt', () {
      final controller = StateController();
      final attempt = controller.setPendingRoom(_roomFixture('idle-reject'));
      // Move to active then explicitly to ending without going through the
      // connected handler.
      controller.promotePendingCallAttempt(attempt);
      controller.beginCallEnding();
      expect(controller.callLifecycle, CallLifecycle.ending);

      expect(controller.handleConnectedEvent(attempt), isFalse,
          reason: 'ending lifecycle must reject a trailing Connected');
      expect(controller.callLifecycle, CallLifecycle.ending);

      controller.endOfCall();
      expect(controller.callLifecycle, CallLifecycle.idle);
      expect(controller.handleConnectedEvent(null), isFalse,
          reason: 'idle lifecycle must reject a stray Connected');
    });

    test('a local hangup that races a trailing backend CallEnded stays silent',
        () {
      final controller = StateController();
      controller.setStatus('Connecting');
      controller.setPendingRoom(_roomFixture('local-hangup'));

      // Widget sets lifecycle to idle before the backend echo of `CallEnded`.
      controller.endOfCall();

      // The `main.dart` gate observes `hasLiveCall == false` and drops the
      // trailing `CallEnded`. The controller's job is to expose that gate
      // state correctly so the late event is silently ignored.
      expect(controller.hasLiveCall, isFalse,
          reason: 'late backend echo after a local hangup must be dropped');
      expect(controller.callLifecycle, CallLifecycle.idle);
    });

    test(
        'pending-contact path: lifecycle moves to connecting, the fake '
        'contact occupies the pending slot, fast CallEnded clears everything, '
        'and a late promotion after endOfCall is skipped', () {
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

      // Production handler invoked by `main.dart` once the gate observes the
      // `CallEnded` event. Dialog surfacing is owned by `main.dart`.
      controller.endOfCall();

      expect(controller.callLifecycle, CallLifecycle.idle,
          reason: 'CallEnded must drop the connecting phase immediately');
      expect(controller.hasLiveCall, isFalse,
          reason: 'gate must close once CallEnded lands');
      expect(controller.pendingContact, isNull,
          reason: 'pending slot must be cleared once CallEnded lands');

      // A late promotion after endOfCall() must be skipped so the
      // call does not resurrect and the timer does not restart.
      controller.promotePendingCallAttempt(controller.currentCallAttempt);
      expect(controller.callLifecycle, CallLifecycle.idle,
          reason: 'post-endOfCall promotion must be skipped');
      expect(controller.pendingContact, isNull,
          reason: 'pending slot stays cleared');
      expect(controller.hasLiveCall, isFalse,
          reason: 'gate must stay closed after a skipped promotion');
    });

    test(
        'fast CallEnded before the start future resumes: promotion '
        'returns false and does not resurrect the call', () {
      // Race scenario: the widget has called setPendingContact, the backend
      // fires a CallEnded callback that endOfCall()s the controller to idle,
      // and then the `await startCall` future resumes. The connected
      // callback's atomic promotion must reject the old attempt so the call
      // does not come back.
      final controller = StateController();
      final contact = FakeContact(
        id: 'fast-end-race',
        contactNickname: 'Race Target',
      );

      controller.setStatus('Connecting');
      final attempt = controller.setPendingContact(contact);
      expect(controller.hasLiveCall, isTrue,
          reason: 'gate must be open while we are negotiating');

      // Production handler: `endOfCall()` is what `main.dart` invokes once
      // the gate observes the `CallEnded` event.
      controller.endOfCall();

      final promoted = controller.promotePendingCallAttempt(attempt);
      expect(promoted, isFalse,
          reason:
              'promote must refuse after a racing endOfCall cleared the lifecycle');
      expect(controller.callLifecycle, CallLifecycle.idle,
          reason: 'lifecycle must stay idle after the racing endOfCall');
      expect(controller.hasLiveCall, isFalse, reason: 'gate must stay closed');
      expect(controller.pendingContact, isNull,
          reason: 'pending slot must be cleared');
    });

    test('a delayed Connected callback cannot promote a newer attempt', () {
      final controller = StateController();
      final firstAttempt = controller.setPendingRoom(_roomFixture('first'));
      controller.endOfCall();
      final secondRoom = _roomFixture('second');
      final secondAttempt = controller.setPendingRoom(secondRoom);

      expect(controller.promotePendingCallAttempt(firstAttempt), isFalse);
      expect(controller.pendingRoom, same(secondRoom));
      expect(controller.callLifecycle, CallLifecycle.connecting);

      expect(controller.promotePendingCallAttempt(secondAttempt), isTrue);
      expect(controller.activeRoom, same(secondRoom));
    });

    test(
        'second call tap during connecting: hasLiveCall guard rejects the tap '
        'and the first call is not disturbed', () {
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

    test('promotion accepts current attempt and rejects stale attempt', () {
      final controller = StateController();
      final first = _roomFixture('promote-1');

      controller.setStatus('Connecting');
      final attempt = controller.setPendingRoom(first);

      final wrongResult = controller.promotePendingCallAttempt(attempt! - 1);
      expect(wrongResult, isFalse, reason: 'promote must refuse stale attempt');
      expect(controller.callLifecycle, CallLifecycle.connecting,
          reason: 'lifecycle must stay connecting on a rejected promote');
      expect(controller.pendingRoom, first,
          reason: 'pending slot must stay intact on a rejected promote');

      final okResult = controller.promotePendingCallAttempt(attempt);
      expect(okResult, isTrue,
          reason: 'promote must accept the same room instance');
      expect(controller.callLifecycle, CallLifecycle.active,
          reason: 'lifecycle must advance to active on success');
      expect(controller.pendingRoom, isNull,
          reason: 'pending slot must be cleared on success');
      expect(controller.activeRoom, first,
          reason: 'active room must be the promoted one');
    });

    test('ending keeps backend-mutating controls blocked until confirmation',
        () {
      final controller = StateController();
      final attempt = controller.setPendingRoom(_roomFixture('ending'));
      controller.promotePendingCallAttempt(attempt);

      expect(controller.beginCallEnding(), isTrue);
      expect(controller.callLifecycle, CallLifecycle.ending);
      expect(controller.blockAudioChanges, isTrue,
          reason:
              'profile and device actions must stay blocked during teardown');

      controller.endOfCall();
      expect(controller.blockAudioChanges, isFalse);
    });
  });
}
