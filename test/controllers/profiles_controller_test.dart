import 'dart:convert';

import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:shared_preferences_platform_interface/in_memory_shared_preferences_async.dart';
import 'package:shared_preferences_platform_interface/shared_preferences_async_platform_interface.dart';
import 'package:telepathy/controllers/profiles_controller.dart';
import 'package:telepathy/core/rust/types.dart';
import 'package:telepathy/models/index.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  setUp(() {
    FlutterSecureStorage.setMockInitialValues(<String, String>{});
    SharedPreferencesAsyncPlatform.instance =
        InMemorySharedPreferencesAsync.empty();
  });

  tearDown(() {
    SharedPreferencesAsyncPlatform.instance = null;
  });

  group('ProfilesController.addRoom', () {
    test('stores its own peer ID list snapshot', () async {
      final controller = ProfilesController(
        storage: const FlutterSecureStorage(),
        options: SharedPreferencesAsync(),
        roomHasher: ({required List<String> peers}) => peers.join('|'),
      );
      controller.profiles['profile-alice'] = Profile(
        id: 'profile-alice',
        nickname: 'Alice Ng',
        peerId: '12D3KooWAliceRoomPeerId1111111111111111111111111111111',
        keypair: const <int>[],
        contacts: <String, Contact>{},
        rooms: <String, Room>{},
      );
      controller.activeProfile = 'profile-alice';

      final peerIds = <String>[
        '12D3KooWAliceRoomPeerId1111111111111111111111111111111',
        '12D3KooWBenRoomPeerId22222222222222222222222222222222',
        '12D3KooWCarlaRoomPeerId333333333333333333333333333333',
      ];
      final expectedPeerIds = List<String>.from(peerIds);

      final room = controller.addRoom('Friday Planning Room', peerIds);
      peerIds.clear();
      await controller.saveRooms();

      expect(room.peerIds, expectedPeerIds);
      expect(controller.rooms[room.id]?.peerIds, expectedPeerIds);
      expect(room.toJson()['peerIds'], expectedPeerIds);
      expect(room.toShareableFormat(), contains(expectedPeerIds.first));
    });
  });

  group('ProfilesController.removeRoom', () {
    test('removes the room, notifies listeners, and persists the change',
        () async {
      const storage = FlutterSecureStorage();
      final controller = ProfilesController(
        storage: storage,
        options: SharedPreferencesAsync(),
        roomHasher: ({required List<String> peers}) => peers.join('|'),
      );
      final room = Room(
        id: 'friday-planning-room',
        peerIds: const <String>[
          '12D3KooWAliceRoomPeerId1111111111111111111111111111111',
          '12D3KooWBenRoomPeerId22222222222222222222222222222222',
        ],
        nickname: 'Friday Planning Room',
      );
      controller.profiles['profile-alice'] = Profile(
        id: 'profile-alice',
        nickname: 'Alice Ng',
        peerId: '12D3KooWAliceRoomPeerId1111111111111111111111111111111',
        keypair: const <int>[],
        contacts: <String, Contact>{},
        rooms: <String, Room>{room.id: room},
      );
      controller.activeProfile = 'profile-alice';
      await controller.saveRooms();

      final persistedBefore = jsonDecode(
        await storage.read(key: 'profile-alice-rooms') ?? '{}',
      ) as Map<String, dynamic>;
      expect(persistedBefore, contains(room.id));

      var notifications = 0;
      controller.addListener(() {
        notifications += 1;
      });

      controller.removeRoom(room);

      expect(controller.rooms, isNot(contains(room.id)));
      expect(notifications, 1);

      await controller.saveRooms();
      final persistedAfter = jsonDecode(
        await storage.read(key: 'profile-alice-rooms') ?? '{}',
      ) as Map<String, dynamic>;
      expect(persistedAfter, isNot(contains(room.id)));
    });

    test('removes an empty room stored under a key that differs from its id',
        () async {
      const storage = FlutterSecureStorage();
      final controller = ProfilesController(
        storage: storage,
        options: SharedPreferencesAsync(),
        roomHasher: ({required List<String> peers}) => peers.join('|'),
      );
      final room = Room(
        id: 'empty-room-hash',
        peerIds: const <String>[],
        nickname: 'Empty Migration Room',
      );
      controller.profiles['profile-alice'] = Profile(
        id: 'profile-alice',
        nickname: 'Alice Ng',
        peerId: '12D3KooWAliceRoomPeerId1111111111111111111111111111111',
        keypair: const <int>[],
        contacts: <String, Contact>{},
        rooms: <String, Room>{'legacy-empty-room-key': room},
      );
      controller.activeProfile = 'profile-alice';
      await controller.saveRooms();

      final persistedBefore = jsonDecode(
        await storage.read(key: 'profile-alice-rooms') ?? '{}',
      ) as Map<String, dynamic>;
      expect(persistedBefore, contains('legacy-empty-room-key'));
      expect(persistedBefore, isNot(contains(room.id)));

      var notifications = 0;
      controller.addListener(() {
        notifications += 1;
      });

      controller.removeRoom(room);

      expect(controller.rooms, isNot(contains('legacy-empty-room-key')));
      expect(notifications, 1);

      await pumpEventQueue();
      final persistedAfter = jsonDecode(
        await storage.read(key: 'profile-alice-rooms') ?? '{}',
      ) as Map<String, dynamic>;
      expect(persistedAfter, isNot(contains('legacy-empty-room-key')));
      expect(persistedAfter, isNot(contains(room.id)));
    });
  });
}
