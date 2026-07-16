import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:telepathy/core/utils/index.dart';
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

  String status = 'Inactive';
  bool _deafened = false;
  bool _muted = false;
  bool inAudioTest = false;
  bool _callEndedRecently = false;
  final Stopwatch _callTimer = Stopwatch();

  /// peerId, status
  final Map<String, SessionStatus> sessions = {};

  ManagerState _sessionManagerState = ManagerState.stopped;
  FrontendNotify? _stopSendingScreenshare;
  FrontendNotify? _stopReceivingScreenshare;
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
  int? setPendingContact(Contact? contact) {
    _pendingContact = contact;
    if (contact != null) {
      _callLifecycle = CallLifecycle.connecting;
      _callAttempt = ++_nextCallAttempt;
    } else {
      _callAttempt = null;
    }
    notifyListeners();
    return _callAttempt;
  }

  /// Records the room the user is attempting to join, before awaiting
  /// `Telepathy.joinRoom()`. This lets the gate in `callState` observe early
  /// failure events while the backend is still negotiating the room.
  int? setPendingRoom(Room? room) {
    _pendingRoom = room;
    if (room != null) {
      _callLifecycle = CallLifecycle.connecting;
      _callAttempt = ++_nextCallAttempt;
    } else {
      _callAttempt = null;
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
      _callAttempt = null;
      _callLifecycle = CallLifecycle.idle;
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

  void screenshareStarted((FrontendNotify stop, bool sending) record) {
    if (record.$2) {
      DebugConsole.log('Sending screenshare started');
      _stopSendingScreenshare = record.$1;
      isSendingScreenshare = true;

      // this catches the sending screenshare being closed by the receiver
      Future.microtask(() async {
        await record.$1.notified();
        // if the screen share is still sending, stop the screenshare
        if (isSendingScreenshare) {
          stopScreenshare(true, true);
        }
      });
    } else {
      DebugConsole.log('Receiving screenshare started');
      _stopReceivingScreenshare = record.$1;
      isReceivingScreenshare = true;
    }

    notifyListeners();
  }

  void stopScreenshare(bool sending, bool notify) {
    DebugConsole.log('Stopping screenshare sending: $sending');

    if (sending) {
      _stopSendingScreenshare?.notify();
      _stopSendingScreenshare = null;
      isSendingScreenshare = false;
    } else {
      _stopReceivingScreenshare?.notify();
      _stopReceivingScreenshare = null;
      isReceivingScreenshare = false;
    }

    if (notify) {
      notifyListeners();
    }
  }

  /// a group of actions run when the call ends
  void endOfCall() {
    _activeRoom?.online.clear();
    _activeContact = null;
    _activeRoom = null;
    _pendingContact = null;
    _pendingRoom = null;
    _callAttempt = null;
    status = 'Inactive';
    _callLifecycle = CallLifecycle.idle;
    _callTimer.stop();
    _callTimer.reset();
    disableCallsTemporarily();
    stopScreenshare(true, false);
    stopScreenshare(false, false);
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
