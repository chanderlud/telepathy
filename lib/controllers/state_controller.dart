import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:telepathy/core/utils/index.dart';
import 'package:telepathy/core/rust/flutter.dart';
import 'package:telepathy/core/rust/types.dart';
import 'package:telepathy/models/index.dart';

/// Lifecycle phases the frontend tracks for an outgoing or room call.
///
/// `Connecting` is the phase after the user requests a call but before the backend
/// has acknowledged the active call (or failed). During this phase, fast `CallEnded`
/// callbacks from the backend must still update the UI rather than be dropped,
/// otherwise users see a hung "Connecting" state with no failure reason.
enum CallLifecycle { idle, connecting, active, ending }

/// A controller which helps bridge the gap between the UI and backend.
class StateController extends ChangeNotifier {
  Contact? _activeContact;
  Room? _activeRoom;
  Contact? _pendingContact;
  Room? _pendingRoom;
  CallLifecycle _callLifecycle = CallLifecycle.idle;
  int? _callAttempt;
  int _nextCallAttempt = 0;
  StartOperation? _startOperation;
  bool _startRequestPending = false;

  String status = 'Inactive';
  bool _deafened = false;
  bool _muted = false;
  bool inAudioTest = false;
  bool _callEndedRecently = false;
  final Stopwatch _callTimer = Stopwatch();

  /// peerId, status
  final Map<String, SessionStatus> sessions = {};

  ManagerState _sessionManagerState = ManagerState.stopped;
  VideoSessionIdentity? _sendingScreenshareIdentity;
  VideoSessionIdentity? _receivingScreenshareIdentity;
  VideoSessionIdentity? _stoppedSendingScreenshareIdentity;
  final Set<VideoSessionIdentity> _terminalSendingScreenshareIdentities = {};
  final Set<VideoSessionIdentity> _terminalReceivingScreenshareIdentities = {};
  bool isSendingScreenshare = false;
  bool isReceivingScreenshare = false;

  Contact? get activeContact => _activeContact;

  Room? get activeRoom => _activeRoom;

  Contact? get pendingContact => _pendingContact;

  Room? get pendingRoom => _pendingRoom;

  CallLifecycle get callLifecycle => _callLifecycle;

  bool get isCallActive => _activeContact != null || _activeRoom != null;

  /// True while a call is being placed (before the backend confirms the call is
  /// active) OR while a call is currently active. Replaces the previous
  /// `_gate_open_until_active` semantics so early `CallEnded` events from the
  /// backend are still observed during the connecting phase.
  bool get hasLiveCall =>
      _callLifecycle == CallLifecycle.connecting ||
      _callLifecycle == CallLifecycle.active ||
      _callLifecycle == CallLifecycle.ending;

  bool get isDeafened => _deafened;

  bool get isMuted => _muted;

  bool get callEndedRecently => _callEndedRecently;

  /// True while audio device changes must be blocked. See [hasLiveCall]
  /// for why this covers the connecting phase too.
  bool get blockAudioChanges => hasLiveCall || inAudioTest;

  ManagerState get sessionManagerState => _sessionManagerState;

  String get callDuration => formatTime(_callTimer.elapsed.inMilliseconds);

  void setActiveContact(Contact? contact) {
    if (contact != null && _callLifecycle == CallLifecycle.idle) {
      // Late promotion after endOfCall already cleared the lifecycle. The
      // call is over; do not resurrect activeContact or restart the timer.
      return;
    }
    _activeContact = contact;
    _pendingContact = null;
    _callLifecycle = contact != null ? CallLifecycle.active : _callLifecycle;
    notifyListeners();
  }

  void setActiveRoom(Room? room) {
    _activeRoom = room;
    _pendingRoom = null;
    _callLifecycle = room != null ? CallLifecycle.active : _callLifecycle;
    notifyListeners();
  }

  /// Records the contact the user is attempting to call, before awaiting
  /// `Telepathy.startCall()`. This lets the gate in `callState` observe early
  /// failure events while the backend is still negotiating the call.
  int? setPendingContact(Contact? contact, [StartOperation? operation]) {
    _pendingContact = contact;
    if (contact != null) {
      _callLifecycle = CallLifecycle.connecting;
      _callAttempt = ++_nextCallAttempt;
      _startOperation = operation;
      _startRequestPending = operation != null;
    } else {
      _callAttempt = null;
      _startOperation = null;
      _startRequestPending = false;
    }
    notifyListeners();
    return _callAttempt;
  }

  /// Records the room the user is attempting to join, before awaiting
  /// `Telepathy.joinRoom()`. This lets the gate in `callState` observe early
  /// failure events while the backend is still negotiating the room.
  int? setPendingRoom(Room? room, [StartOperation? operation]) {
    _pendingRoom = room;
    if (room != null) {
      _callLifecycle = CallLifecycle.connecting;
      _callAttempt = ++_nextCallAttempt;
      _startOperation = operation;
      _startRequestPending = operation != null;
    } else {
      _callAttempt = null;
      _startOperation = null;
      _startRequestPending = false;
    }
    notifyListeners();
    return _callAttempt;
  }

  /// Atomically promotes current pending target only for [attempt].
  ///
  /// The backend callback captures [currentCallAttempt] before any await, then
  /// calls this method before loading its optional sound. A stale callback cannot
  /// promote a later attempt or resurrect one that is ending.
  bool promotePendingCallAttempt(int? attempt) {
    if (attempt == null ||
        attempt != _callAttempt ||
        _callLifecycle != CallLifecycle.connecting) {
      return false;
    }

    final contact = _pendingContact;
    final room = _pendingRoom;
    if (contact == null && room == null) {
      return false;
    }

    _activeContact = contact;
    _activeRoom = room;
    _pendingContact = null;
    _pendingRoom = null;
    _callLifecycle = CallLifecycle.active;
    status = 'Active';
    _callTimer.start();
    notifyListeners();
    return true;
  }

  int? get currentCallAttempt => _callAttempt;

  /// Whether [attempt] still owns the current call lifecycle.
  bool isCurrentCallAttempt(int? attempt) =>
      attempt != null && attempt == _callAttempt;

  /// Handles the lifecycle transition for an incoming `CallState_Connected`
  /// event captured with [attempt]. Returns whether the caller should
  /// continue the connected path (e.g. play the connected sound).
  ///
  /// Returns `true` when:
  /// - [attempt] still owns the lifecycle and is being promoted from
  ///   `connecting` to `active` for the first time, OR
  /// - the same attempt is already `active` because a `Waiting` event
  ///   promoted the room first, in which case the status is reset to
  ///   `Active` so the stale `Waiting for peers` label does not linger
  ///   and the connected-sound path runs.
  ///
  /// Returns `false` when the event is stale: lifecycle is `idle` or
  /// `ending`, or [attempt] no longer owns the current call (so a delayed
  /// callback from a previous attempt cannot resurrect or disturb it).
  bool handleConnectedEvent(int? attempt) {
    if (!isCurrentCallAttempt(attempt)) {
      return false;
    }
    switch (_callLifecycle) {
      case CallLifecycle.idle:
      case CallLifecycle.ending:
        return false;
      case CallLifecycle.connecting:
        return promotePendingCallAttempt(attempt);
      case CallLifecycle.active:
        // The room may already be active because a `Waiting` event promoted
        // the pending slot earlier. Reset the status to `Active` so the
        // stale `Waiting for peers` label does not linger, and let the
        // connected sound play.
        status = 'Active';
        notifyListeners();
        return true;
    }
  }

  bool get isStartRequestPending => _startRequestPending;

  /// Cancels the operation that owns the current pending start request.
  /// The attempt remains installed until its request future has settled.
  void cancelCurrentStartOperation() {
    _startOperation?.cancel();
  }

  /// Settles the request associated with [attempt]. A locally cancelled start
  /// remains in `ending` until this point so a delayed backend acquisition is
  /// still cancelled by its original operation handle.
  void settleStartAttempt(int? attempt) {
    if (!isCurrentCallAttempt(attempt)) return;
    _startOperation = null;
    _startRequestPending = false;
    if (_callLifecycle == CallLifecycle.ending) {
      endOfCall();
    }
  }

  /// Clear any pending call target that has not yet transitioned to active.
  /// Called by the front-end after a fast failure or a transition to active so
  /// the pending slot doesn't leak into another call attempt.
  void clearPending() {
    if (_pendingContact != null || _pendingRoom != null) {
      _pendingContact = null;
      _pendingRoom = null;
      notifyListeners();
    }
  }

  /// Starts local teardown without reopening backend-mutating controls.
  bool beginCallEnding() {
    if (_callLifecycle == CallLifecycle.idle ||
        _callLifecycle == CallLifecycle.ending) {
      return false;
    }
    _callLifecycle = CallLifecycle.ending;
    notifyListeners();
    return true;
  }

  void setStatus(String status) {
    this.status = status;

    if (status == 'Inactive') {
      _activeContact = null;
      _activeRoom = null;
      _pendingContact = null;
      _pendingRoom = null;
      if (!_startRequestPending) {
        _callAttempt = null;
        _startOperation = null;
      }
      _callLifecycle =
          _startRequestPending ? CallLifecycle.ending : CallLifecycle.idle;
      _callTimer.stop();
      _callTimer.reset();
    } else if (status == 'Active') {
      _callLifecycle = CallLifecycle.active;
      _callTimer.start();
    }

    notifyListeners();
  }

  /// called when the session manager state changes
  void setSessionManager(ManagerState state) {
    _sessionManagerState = state;
    notifyListeners();
  }

  bool isActiveContact(Contact contact) {
    return _activeContact?.id() == contact.id();
  }

  bool isActiveRoom(Room room) {
    return _activeRoom?.id == room.id;
  }

  void roomJoin(String peerId) {
    _activeRoom?.online.add(peerId);
    notifyListeners();
  }

  void roomLeave(String peerId) {
    _activeRoom?.online.remove(peerId);
    notifyListeners();
  }

  bool isOnlineContact(Contact contact) {
    return sessionStatus(contact).runtimeType == SessionStatus_Connected;
  }

  /// called when a session changes status
  void updateSession((String peerId, SessionStatus status) record) {
    sessions[record.$1] = record.$2;
    notifyListeners();
  }

  SessionStatus sessionStatus(Contact contact) {
    return sessions[contact.peerId()] ?? const SessionStatus.unknown();
  }

  void deafen() {
    _deafened = !_deafened;
    _muted = _deafened;
    notifyListeners();
  }

  void mute() {
    _muted = !_muted;
    notifyListeners();
  }

  void setInAudioTest(bool active) {
    inAudioTest = active;
    status = inAudioTest ? 'In Audio Test' : 'Inactive';

    notifyListeners();
  }

  Future<void> runAudioTest(Future<void> Function() audioTest) async {
    setInAudioTest(true);

    try {
      await audioTest();
    } finally {
      setInAudioTest(false);
    }
  }

  void disableCallsTemporarily() {
    _callEndedRecently = true;

    Timer(const Duration(seconds: 1), () {
      _callEndedRecently = false;
    });
  }

  void handleVideoLifecycle(VideoLifecycleEvent event) {
    if (event.source != VideoSource.display) return;

    if (event.phase == VideoPhase.active) {
      if (!isCallActive) return;
      if (event.role == VideoRole.sender) {
        if (event.identity == _stoppedSendingScreenshareIdentity) return;
        if (_terminalSendingScreenshareIdentities.contains(event.identity)) {
          return;
        }
        _terminalSendingScreenshareIdentities.remove(event.identity);
        _sendingScreenshareIdentity = event.identity;
        isSendingScreenshare = true;
      } else {
        if (_terminalReceivingScreenshareIdentities.contains(event.identity)) {
          return;
        }
        _terminalReceivingScreenshareIdentities.remove(event.identity);
        _receivingScreenshareIdentity = event.identity;
        isReceivingScreenshare = true;
      }
      notifyListeners();
      return;
    }

    if (event.phase != VideoPhase.terminal) return;

    var handled = false;
    if (event.role == VideoRole.sender) {
      _terminalSendingScreenshareIdentities.add(event.identity);
      if (event.identity == _sendingScreenshareIdentity) {
        _sendingScreenshareIdentity = null;
        isSendingScreenshare = false;
        handled = true;
      }
      if (event.identity == _stoppedSendingScreenshareIdentity) {
        _stoppedSendingScreenshareIdentity = null;
        handled = true;
      }
    } else {
      _terminalReceivingScreenshareIdentities.add(event.identity);
      if (event.identity == _receivingScreenshareIdentity) {
        _receivingScreenshareIdentity = null;
        isReceivingScreenshare = false;
        handled = true;
      }
    }

    if (!handled) return;
    notifyListeners();
  }

  VideoSessionIdentity? stopSendingScreenshare() {
    final identity = _sendingScreenshareIdentity;
    if (identity == null) return null;

    _sendingScreenshareIdentity = null;
    _stoppedSendingScreenshareIdentity = identity;
    isSendingScreenshare = false;
    notifyListeners();
    return identity;
  }

  void clearScreenshares() {
    _sendingScreenshareIdentity = null;
    _receivingScreenshareIdentity = null;
    _stoppedSendingScreenshareIdentity = null;
    _terminalSendingScreenshareIdentities.clear();
    _terminalReceivingScreenshareIdentities.clear();
    isSendingScreenshare = false;
    isReceivingScreenshare = false;
  }

  /// A group of actions run when the call ends.
  void endOfCall() {
    _activeRoom?.online.clear();
    _activeContact = null;
    _activeRoom = null;
    _callTimer.stop();
    _callTimer.reset();
    clearScreenshares();

    if (_startRequestPending) {
      // Keep the target, attempt, and operation until the original start future
      // settles. Its cancellation handle must remain associated with the exact
      // request that could still acquire backend ownership.
      status = 'Ending';
      _callLifecycle = CallLifecycle.ending;
      notifyListeners();
      return;
    }

    _pendingContact = null;
    _pendingRoom = null;
    _callAttempt = null;
    _startOperation = null;
    status = 'Inactive';
    _callLifecycle = CallLifecycle.idle;
    disableCallsTemporarily();
    notifyListeners();
  }
}

/// Notifies listeners every second.
class PeriodicNotifier extends ChangeNotifier {
  Timer? _timer;

  PeriodicNotifier() {
    _timer = Timer.periodic(const Duration(seconds: 1), (timer) {
      notifyListeners();
    });
  }

  @override
  void dispose() {
    _timer?.cancel();
    _timer = null;
    super.dispose();
  }
}
