import 'dart:async';
import 'dart:typed_data';

import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/core/rust/flutter.dart';
import 'package:telepathy/core/rust/lib.dart';
import 'package:telepathy/core/rust/player.dart';
import 'package:telepathy/core/rust/types.dart';
import 'package:telepathy/models/index.dart';

/// Mock backend for live UI development and QA without the Rust core.
///
/// `lib/mock_main.dart` boots the real app UI against these fakes, so every
/// widget, controller, and lifecycle path is exercised exactly as in
/// production — only the bridge boundary is substituted. Run it with:
///
/// ```sh
/// flutter run -d linux --target=lib/mock_main.dart \
///   --dart-define=MOCK_SCENARIO=demo
/// ```
///
/// Scenarios: `demo` (contacts, rooms, mixed session states), `room-active`
/// (already inside a room call), `empty` (fresh profile, no contacts).

/// Synthetic peer ids used by the mock scenarios. They are deliberately not
/// valid iroh public keys; the mock validator accepts them.
class MockPeers {
  static const String self = 'mock-peer-self';
  static const String ada = 'mock-peer-ada';
  static const String grace = 'mock-peer-grace';
  static const String alan = 'mock-peer-alan';
  static const String edsger = 'mock-peer-edsger';
  static const String margaret = 'mock-peer-margaret';
}

/// A pure-Dart [Contact] with mutable fields so the edit dialog behaves
/// exactly like it does against the Rust-backed contact.
class MockContact implements Contact {
  MockContact({required String peerId, required String nickname})
      : _peerId = peerId,
        _nickname = nickname;

  final String _peerId;
  String _nickname;
  double _outputVolume = 0.0;

  @override
  String id() => _peerId;

  @override
  String peerId() => _peerId;

  @override
  Future<PublicKey> getPeerId() async => MockPublicKey();

  @override
  bool idEq({required List<int> id}) => false;

  @override
  String nickname() => _nickname;

  @override
  void setNickname({required String nickname}) => _nickname = nickname;

  @override
  double outputVolume() => _outputVolume;

  @override
  void setOutputVolume({required double decibel}) => _outputVolume = decibel;

  @override
  Contact pubClone() => this;

  @override
  void dispose() {}

  @override
  bool get isDisposed => false;
}

class MockPublicKey implements PublicKey {
  @override
  void dispose() {}

  @override
  bool get isDisposed => false;
}

class MockStartOperation implements StartOperation {
  bool cancelled = false;

  @override
  void cancel() => cancelled = true;

  @override
  void dispose() {}

  @override
  bool get isDisposed => false;
}

class MockArcHost implements ArcHost {
  @override
  void dispose() {}

  @override
  bool get isDisposed => false;
}

class MockSoundHandle implements FlutterSoundHandle {
  @override
  void cancel() {}

  @override
  void dispose() {}

  @override
  bool get isDisposed => false;
}

class MockSoundPlayer implements SoundPlayer {
  final MockSoundHandle _handle = MockSoundHandle();

  @override
  ArcHost host() => MockArcHost();

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

/// Simulates the backend's call lifecycle: session connects, outgoing calls
/// and room joins succeed after a short delay, and room members trickle in —
/// driving the [StateController] through the same public transitions the
/// real `callState`/`sessionStatus` callbacks use in `main.dart`.
class MockTelepathy implements Telepathy {
  MockTelepathy({required StateController stateController})
      : _stateController = stateController;

  final StateController _stateController;

  @override
  StartOperation newStartOperation() => MockStartOperation();

  @override
  Future<void> startSession({required Contact contact}) async {
    final String peerId = contact.peerId();
    _stateController.updateSession((peerId, const SessionStatus.connecting()));
    await Future<void>.delayed(const Duration(milliseconds: 1500));
    _stateController.updateSession((
      peerId,
      const SessionStatus.connected(
          relayed: false, remoteAddress: '192.168.1.42:4160'),
    ));
  }

  @override
  Future<void> stopSession({required Contact contact}) async {
    _stateController
        .updateSession((contact.peerId(), const SessionStatus.inactive()));
  }

  @override
  Future<void> startCall(
      {required Contact contact, required StartOperation operation}) async {
    await Future<void>.delayed(const Duration(milliseconds: 900));
    if ((operation as MockStartOperation).cancelled) {
      throw const DartError(message: 'Call was cancelled');
    }
    _stateController.handleConnectedEvent(_stateController.currentCallAttempt);
  }

  @override
  Future<void> joinRoom(
      {required List<String> memberStrings,
      required StartOperation operation}) async {
    await Future<void>.delayed(const Duration(milliseconds: 900));
    if ((operation as MockStartOperation).cancelled) {
      throw const DartError(message: 'Room join was cancelled');
    }

    final int? attempt = _stateController.currentCallAttempt;
    // Mirror the real Waiting -> Connected -> RoomJoin event sequence.
    _stateController.promotePendingCallAttempt(attempt);
    _stateController.setStatus('Waiting for peers');

    final List<String> others =
        memberStrings.where((p) => p != MockPeers.self).toList();
    for (final String peer in others) {
      await Future<void>.delayed(const Duration(milliseconds: 1200));
      if (_stateController.activeRoom == null) return; // hung up meanwhile
      _stateController.handleConnectedEvent(attempt);
      _stateController.roomJoin(peer);
    }
  }

  @override
  Future<void> endCall() async {}

  @override
  Future<(List<AudioDevice>, List<AudioDevice>)> listDevices() async =>
      (<AudioDevice>[], <AudioDevice>[]);

  @override
  Future<void> audioTest() async {}

  @override
  ChatMessage buildChat(
          {required Contact contact,
          required String text,
          required List<(String, Uint8List)> attachments}) =>
      throw UnimplementedError('chat is not implemented in mock mode');

  @override
  void pauseStatistics() {}

  @override
  void resumeStatistics() {}

  @override
  Future<void> restartManager() async {
    _stateController.setSessionManager(ManagerState.starting);
    await Future<void>.delayed(const Duration(milliseconds: 800));
    _stateController.setSessionManager(ManagerState.active);
  }

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
  Future<void> startManager() async {
    _stateController.setSessionManager(ManagerState.active);
  }

  @override
  Future<void> startScreenshare({required Contact contact}) async {}

  @override
  Future<PreparedIdentitySwitch> prepareIdentitySwitch(
          {required List<int> targetKey,
          required List<Contact> targetContacts}) =>
      throw UnimplementedError('identity switch is not implemented in mock');

  @override
  void dispose() {}

  @override
  bool get isDisposed => false;
}

/// Everything [mock_main] needs to build the provider tree.
class MockAppContext {
  MockAppContext({
    required this.profilesController,
    required this.stateController,
    required this.telepathy,
    required this.soundPlayer,
  });

  final ProfilesController profilesController;
  final StateController stateController;
  final MockTelepathy telepathy;
  final MockSoundPlayer soundPlayer;
}

/// Boots the mock backend: creates a demo profile in secure storage, seeds
/// contacts/rooms/session states for [scenario], and returns the wired
/// controllers. [options] must be an in-memory preferences instance so the
/// mock profile never touches real user data.
Future<MockAppContext> createMockAppContext({
  required String scenario,
  required SharedPreferencesAsync options,
}) async {
  const String profileId = 'mock-profile';
  const FlutterSecureStorage storage = FlutterSecureStorage();

  await storage.write(
    key: '$profileId-keypair',
    value: 'AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=', // 32 bytes, base64
  );
  await storage.write(key: '$profileId-peerId', value: MockPeers.self);
  await storage.write(key: '$profileId-nickname', value: 'Mock User');
  await storage.write(key: '$profileId-contacts', value: '{}');
  await storage.write(key: '$profileId-rooms', value: '{}');
  await options.setStringList('profilesV2', const <String>[profileId]);
  await options.setString('activeProfile', profileId);

  final ProfilesController profilesController = ProfilesController(
    storage: storage,
    options: options,
    roomHasher: ({required List<String> peers}) =>
        'room-${Object.hashAllUnordered(peers)}',
    contactFactory: ({required nickname, required peerId}) =>
        MockContact(peerId: peerId, nickname: nickname),
    peerIdValidator: (String peerId) => peerId.trim().isNotEmpty,
  );
  await profilesController.init(const <String>[]);

  final StateController stateController = StateController();
  final MockTelepathy telepathy =
      MockTelepathy(stateController: stateController);

  if (scenario != 'empty') {
    _seedDemoData(profilesController, stateController);
  }

  if (scenario == 'room-active') {
    // Enter the first room as if the user had just joined it: pending ->
    // active, then peers join. Deferred so the first frame can build.
    final Room room = profilesController.rooms.values.first;
    Timer(const Duration(milliseconds: 400), () {
      stateController.setPendingRoom(room);
      final int? attempt = stateController.currentCallAttempt;
      stateController.promotePendingCallAttempt(attempt);
      stateController.setStatus('Active');
      stateController.roomJoin(MockPeers.ada);
      stateController.roomJoin(MockPeers.grace);
    });
  }

  return MockAppContext(
    profilesController: profilesController,
    stateController: stateController,
    telepathy: telepathy,
    soundPlayer: MockSoundPlayer(),
  );
}

void _seedDemoData(
    ProfilesController profilesController, StateController stateController) {
  final Map<String, Contact> contacts = profilesController.contacts;
  for (final MockContact contact in [
    MockContact(peerId: MockPeers.ada, nickname: 'Ada Lovelace'),
    MockContact(peerId: MockPeers.grace, nickname: 'Grace Hopper'),
    MockContact(peerId: MockPeers.alan, nickname: 'Alan Turing'),
    MockContact(peerId: MockPeers.edsger, nickname: 'Edsger Dijkstra'),
    MockContact(peerId: MockPeers.margaret, nickname: 'Margaret Hamilton'),
  ]) {
    contacts[contact.id()] = contact;
  }

  stateController.updateSession((
    MockPeers.ada,
    const SessionStatus.connected(
        relayed: false, remoteAddress: '192.168.1.42:4160'),
  ));
  stateController.updateSession((
    MockPeers.grace,
    const SessionStatus.connected(
        relayed: true, remoteAddress: 'usw1-1.relay.iroh.network'),
  ));
  stateController
      .updateSession((MockPeers.alan, const SessionStatus.connecting()));
  // Edsger has no session entry -> unknown/offline.
  stateController
      .updateSession((MockPeers.margaret, const SessionStatus.inactive()));

  profilesController.addRoom(
      'Weekend Gaming', [MockPeers.self, MockPeers.ada, MockPeers.grace]);
  profilesController.addRoom('Team Standup',
      [MockPeers.self, MockPeers.ada, MockPeers.grace, MockPeers.alan]);

  stateController.setSessionManager(ManagerState.active);
}
