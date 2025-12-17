import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:telepathy/core/utils/index.dart';
import 'package:telepathy/src/rust/flutter.dart';
import 'package:telepathy/models/index.dart';

/// A controller which helps bridge the gap between the UI and backend.
class StateController extends ChangeNotifier {
  Contact? _activeContact;
  Room? _activeRoom;

  String status = 'Inactive';
  bool _deafened = false;
  bool _muted = false;
  bool inAudioTest = false;
  bool _callEndedRecently = false;
  final Stopwatch _callTimer = Stopwatch();

  /// peerId, status
  final Map<String, SessionStatus> sessions = {};

  /// active, restartable
  (bool, bool) _sessionManager = (false, false);

  DartNotify? _stopSendingScreenshare;
  DartNotify? _stopReceivingScreenshare;
  bool isSendingScreenshare = false;
  bool isReceivingScreenshare = false;

  Contact? get activeContact => _activeContact;

  Room? get activeRoom => _activeRoom;

  bool get isCallActive => _activeContact != null || _activeRoom != null;

  bool get isDeafened => _deafened;

  bool get isMuted => _muted;

  bool get callEndedRecently => _callEndedRecently;

  bool get blockAudioChanges => isCallActive || inAudioTest;

  bool get sessionManagerActive => _sessionManager.$1;

  bool get sessionManagerRestartable => _sessionManager.$2;

  String get callDuration => formatTime(_callTimer.elapsed.inMilliseconds);

  void setActiveContact(Contact? contact) {
    _activeContact = contact;
    notifyListeners();
  }

  void setActiveRoom(Room? room) {
    _activeRoom = room;
    notifyListeners();
  }

  void setStatus(String status) {
    this.status = status;

    if (status == 'Inactive') {
      _activeContact = null;
      _activeRoom = null;
      _callTimer.stop();
      _callTimer.reset();
    } else if (status == 'Active') {
      _callTimer.start();
    }

    notifyListeners();
  }

  /// called when the session manager state changes
  void setSessionManager((bool active, bool restartable) record) {
    _sessionManager = record;
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
    return sessions[contact.peerId()] ?? SessionStatus.unknown();
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

  void setInAudioTest() {
    inAudioTest = !inAudioTest;
    status = inAudioTest ? 'In Audio Test' : 'Inactive';

    notifyListeners();
  }

  void disableCallsTemporarily() {
    _callEndedRecently = true;

    Timer(const Duration(seconds: 1), () {
      _callEndedRecently = false;
    });
  }

  void screenshareStarted((DartNotify stop, bool sending) record) {
    if (record.$2) {
      DebugConsole.log('Sending screenshare started');
      _stopSendingScreenshare = record.$1;
      isSendingScreenshare = true;

      // this catches the sending screenshare being closed by the receiver
      Future.microtask(() async {
        await record.$1.notified();
        // if the screen share is still sending, stop the screenshare
        if (isSendingScreenshare) {
          stopScreenshare(true);
        }
      });
    } else {
      DebugConsole.log('Receiving screenshare started');
      _stopReceivingScreenshare = record.$1;
      isReceivingScreenshare = true;
    }

    notifyListeners();
  }

  void stopScreenshare(bool sending) {
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

    notifyListeners();
  }

  /// a group of actions run when the call ends
  void endOfCall() {
    _activeRoom?.online.clear();
    setActiveContact(null);
    setActiveRoom(null);
    setStatus('Inactive');
    disableCallsTemporarily();
    stopScreenshare(true);
    stopScreenshare(false);
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
