import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';

import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:shared_preferences_platform_interface/in_memory_shared_preferences_async.dart';
import 'package:shared_preferences_platform_interface/shared_preferences_async_platform_interface.dart';
import 'package:telepathy/controllers/profiles_controller.dart';
import 'package:telepathy/core/rust/frb_generated.dart'
    show RustLib, RustLibApi;
import 'package:telepathy/core/rust/flutter.dart';
import 'package:telepathy/core/rust/lib.dart';
import 'package:telepathy/core/rust/types.dart';
import 'package:telepathy/models/index.dart';

/// Fixture tuple for [seedController]: a profile's persisted id, nickname,
/// peerId, and keypair bytes. Keypair MUST be 32 bytes so it passes the
/// production `_loadProfile` length validation.
typedef SeedProfile = ({
  String id,
  String nickname,
  String peerId,
  List<int> keypair,
});

/// Seeds the supplied profiles + active-profile id into real
/// `FlutterSecureStorage` + `SharedPreferencesAsync`, constructs a fresh
/// [ProfilesController], and runs `init()` so the controller's in-memory
/// state reflects realistic persisted fixtures rather than test-only
/// mutations. Production code is exercised end-to-end from storage to
/// memory.
///
/// `roomHasher` overrides the production Rust-backed hash so tests that
/// subsequently call `addRoom` can do so without the Rust runtime loaded.
/// Init-time room loading is still not possible because `Room.fromJson`
/// calls the global Rust `roomHash` directly; tests that need pre-seeded
/// rooms must add them via `controller.addRoom` (with the override) or
/// accept that init() will produce an empty rooms map.
Future<ProfilesController> seedController({
  required FlutterSecureStorage storage,
  required SharedPreferencesAsync options,
  required List<SeedProfile> profiles,
  required String activeProfileId,
  RoomHasher? roomHasher,
}) async {
  for (final p in profiles) {
    await storage.write(
      key: '${p.id}-keypair',
      value: base64Encode(p.keypair),
    );
    await storage.write(key: '${p.id}-peerId', value: p.peerId);
    await storage.write(key: '${p.id}-nickname', value: p.nickname);
  }
  await options.setStringList(
    'profilesV2',
    profiles.map((p) => p.id).toList(),
  );
  await options.setString('activeProfile', activeProfileId);
  final controller = ProfilesController(
    storage: storage,
    options: options,
    roomHasher:
        roomHasher ?? (({required List<String> peers}) => peers.join('|')),
  );
  await controller.init(const <String>[]);
  return controller;
}

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
      final controller = await seedController(
        storage: const FlutterSecureStorage(),
        options: SharedPreferencesAsync(),
        profiles: const <SeedProfile>[
          (
            id: 'profile-alice',
            nickname: 'Alice Ng',
            peerId: '12D3KooWAliceRoomPeerId1111111111111111111111111111111',
            keypair: <int>[
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
            ],
          ),
        ],
        activeProfileId: 'profile-alice',
      );

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
      final options = SharedPreferencesAsync();
      // init() loads profile-alice with an empty rooms map (Room.fromJson
      // requires the Rust runtime, unavailable in unit tests). The room
      // is added through the public `addRoom` API so roomHasher (overridden
      // in seedController) handles the id computation.
      final controller = await seedController(
        storage: storage,
        options: options,
        profiles: const <SeedProfile>[
          (
            id: 'profile-alice',
            nickname: 'Alice Ng',
            peerId: '12D3KooWAliceRoomPeerId1111111111111111111111111111111',
            keypair: <int>[
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
            ],
          ),
        ],
        activeProfileId: 'profile-alice',
      );
      final room = controller.addRoom(
        'Friday Planning Room',
        const <String>[
          '12D3KooWAliceRoomPeerId1111111111111111111111111111111',
          '12D3KooWBenRoomPeerId22222222222222222222222222222222',
        ],
      );
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
      final options = SharedPreferencesAsync();
      final controller = await seedController(
        storage: storage,
        options: options,
        profiles: const <SeedProfile>[
          (
            id: 'profile-alice',
            nickname: 'Alice Ng',
            peerId: '12D3KooWAliceRoomPeerId1111111111111111111111111111111',
            keypair: <int>[
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
            ],
          ),
        ],
        activeProfileId: 'profile-alice',
      );
      // Seed a legacy rooms record under a key that does not match the
      // room id, mirroring the historical migration scenario. The room is
      // added directly to the in-memory map because Room.fromJson (which
      // init() would call) requires the Rust runtime.
      const String legacyKey = 'legacy-empty-room-key';
      final room = Room(
        id: 'empty-room-hash',
        peerIds: const <String>[],
        nickname: 'Empty Migration Room',
      );
      controller.profiles['profile-alice']!.rooms[legacyKey] = room;
      await controller.saveRooms();

      final persistedBefore = jsonDecode(
        await storage.read(key: 'profile-alice-rooms') ?? '{}',
      ) as Map<String, dynamic>;
      expect(persistedBefore, contains(legacyKey));
      expect(persistedBefore, isNot(contains(room.id)));

      var notifications = 0;
      controller.addListener(() {
        notifications += 1;
      });

      controller.removeRoom(room);

      expect(controller.rooms, isNot(contains(legacyKey)));
      expect(notifications, 1);

      await pumpEventQueue();
      final persistedAfter = jsonDecode(
        await storage.read(key: 'profile-alice-rooms') ?? '{}',
      ) as Map<String, dynamic>;
      expect(persistedAfter, isNot(contains(legacyKey)));
      expect(persistedAfter, isNot(contains(room.id)));
    });
  });

  group('ProfilesController two-phase identity-switch transaction', () {
    /// Subclass of [SharedPreferencesAsync] that can be staged to throw on
    /// `setString` or `setStringList`. Used by the two-phase transaction
    /// tests to drive the REAL [ProfilesController] through its rollback
    /// paths without test-only hooks in the production controller.
    _ThrowingSharedPreferences buildOptions() => _ThrowingSharedPreferences();

    Future<ProfilesController> seedTwo({
      required FlutterSecureStorage storage,
      required SharedPreferencesAsync options,
      String active = 'profile-alpha',
    }) {
      return seedController(
        storage: storage,
        options: options,
        profiles: const <SeedProfile>[
          (
            id: 'profile-alpha',
            nickname: 'Alpha',
            peerId: '12D3KooWAlphaPeerId000000000000000000000000000000000',
            keypair: <int>[
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
              1,
            ],
          ),
          (
            id: 'profile-beta',
            nickname: 'Beta',
            peerId: '12D3KooWBetaPeerId11111111111111111111111111111111',
            keypair: <int>[
              2,
              2,
              2,
              2,
              2,
              2,
              2,
              2,
              2,
              2,
              2,
              2,
              2,
              2,
              2,
              2,
              2,
              2,
              2,
              2,
              2,
              2,
              2,
              2,
              2,
              2,
              2,
              2,
              2,
              2,
              2,
              2,
            ],
          ),
          (
            id: 'profile-gamma',
            nickname: 'Gamma',
            peerId: '12D3KooWGammaPeerId22222222222222222222222222222222',
            keypair: <int>[
              3,
              3,
              3,
              3,
              3,
              3,
              3,
              3,
              3,
              3,
              3,
              3,
              3,
              3,
              3,
              3,
              3,
              3,
              3,
              3,
              3,
              3,
              3,
              3,
              3,
              3,
              3,
              3,
              3,
              3,
              3,
              3,
            ],
          ),
        ],
        activeProfileId: active,
      );
    }

    Future<
        ({
          _MutableContact contact,
          ProfilesController controller,
          Room room,
        })> seedActiveMutations({
      required FlutterSecureStorage storage,
      required SharedPreferencesAsync options,
    }) async {
      final controller = await seedTwo(storage: storage, options: options);
      final contact = _MutableContact(
        id: 'contact-alpha',
        nickname: 'Alice Contact',
        peerId: '12D3KooWContactAlpha000000000000000000000000000000',
      );
      controller.profiles['profile-alpha']!.contacts[contact.id()] = contact;
      final room = controller.addRoom(
        'Alpha Planning Room',
        const <String>[
          '12D3KooWAlphaPeerId000000000000000000000000000000000',
          '12D3KooWContactAlpha000000000000000000000000000000',
        ],
      );
      await controller.saveContacts();
      await controller.saveRooms();
      return (contact: contact, controller: controller, room: room);
    }

    void expectContactAndRoomMutationsBlocked({
      required ProfilesController controller,
      required Contact contact,
      required Room room,
    }) {
      final attempts = <({String name, Object? Function() invoke})>[
        (
          name: 'addContact',
          invoke: () => controller.addContact(
                'Blocked Contact',
                '12D3KooWBlockedContact00000000000000000000000000000',
              ),
        ),
        (
          name: 'tryAddContact',
          invoke: () => controller.tryAddContact(
                'Blocked Contact',
                '12D3KooWBlockedContact00000000000000000000000000000',
              ),
        ),
        (
          name: 'updateContact',
          invoke: () => controller.updateContact(
                contact,
                nickname: 'Blocked Contact Rename',
                outputVolume: 7.5,
              ),
        ),
        (
          name: 'removeContact',
          invoke: () => controller.removeContact(contact),
        ),
        (name: 'saveContacts', invoke: controller.saveContacts),
        (
          name: 'addRoom',
          invoke: () => controller.addRoom(
                'Blocked Room',
                const <String>[
                  '12D3KooWAlphaPeerId000000000000000000000000000000000',
                  '12D3KooWBlockedRoomPeer000000000000000000000000000000',
                ],
              ),
        ),
        (
          name: 'tryAddRoom',
          invoke: () => controller.tryAddRoom(
                'Blocked Room',
                const <String>[
                  '12D3KooWAlphaPeerId000000000000000000000000000000000',
                  '12D3KooWBlockedRoomPeer000000000000000000000000000000',
                ],
              ),
        ),
        (
          name: 'updateRoom',
          invoke: () => controller.updateRoom(
                room,
                nickname: 'Blocked Room Rename',
              ),
        ),
        (
          name: 'removeRoom',
          invoke: () => controller.removeRoom(room),
        ),
        (name: 'saveRooms', invoke: controller.saveRooms),
      ];

      for (final attempt in attempts) {
        expect(
          attempt.invoke,
          throwsA(isA<StateError>()),
          reason:
              '${attempt.name} must reject while identity switch is pending',
        );
      }
    }

    test(
        'switchActiveProfile blocks every contact and room mutation while '
        'begin is pending, snapshots after closing the gate, then commits',
        () async {
      const storage = FlutterSecureStorage();
      final options = buildOptions();
      final seeded = await seedActiveMutations(
        storage: storage,
        options: options,
      );
      final controller = seeded.controller;
      addTearDown(controller.dispose);

      bool? pendingWhenTargetWasCloned;
      final targetContact = _MutableContact(
        id: 'contact-beta',
        nickname: 'Beta Contact',
        peerId: '12D3KooWContactBeta0000000000000000000000000000000',
        onClone: () {
          pendingWhenTargetWasCloned = controller.isIdentitySwitchPending;
        },
      );
      controller.profiles['profile-beta']!.contacts[targetContact.id()] =
          targetContact;

      final contactNicknameBefore = seeded.contact.nickname();
      final contactVolumeBefore = seeded.contact.outputVolume();
      final roomNicknameBefore = seeded.room.nickname;
      final contactsBefore = Map<String, Contact>.from(controller.contacts);
      final roomsBefore = Map<String, Room>.from(controller.rooms);
      final storageBefore = await storage.readAll();

      final telepathy = _RecordingTelepathy()..pauseBeginIdentitySwitch();
      final switchFuture = controller.switchActiveProfile(
        'profile-beta',
        telepathy: telepathy,
      );
      await telepathy.beginIdentitySwitchEntered.future;

      expect(pendingWhenTargetWasCloned, isTrue,
          reason: 'the Dart mutation gate must close before traversing the '
              'target contact map to build the backend snapshot');
      expect(controller.isIdentitySwitchPending, isTrue);
      expect(telepathy.beginCalls.single.contacts, hasLength(1));
      expect(
        telepathy.beginCalls.single.contacts.single.nickname(),
        'Beta Contact',
      );

      expectContactAndRoomMutationsBlocked(
        controller: controller,
        contact: seeded.contact,
        room: seeded.room,
      );
      await pumpEventQueue();

      expect(controller.activeProfile, 'profile-alpha');
      expect(controller.contacts, contactsBefore);
      expect(controller.rooms, roomsBefore);
      expect(seeded.contact.nickname(), contactNicknameBefore);
      expect(seeded.contact.outputVolume(), contactVolumeBefore);
      expect(seeded.room.nickname, roomNicknameBefore);
      expect(await storage.readAll(), storageBefore,
          reason: 'rejected persistence calls must not enqueue writes while '
              'the backend begin future is paused');

      telepathy.releaseBeginIdentitySwitch();
      await switchFuture;

      expect(controller.activeProfile, 'profile-beta');
      expect(controller.isIdentitySwitchPending, isFalse);
      expect(telepathy.commitCalls, hasLength(1));
      expect(telepathy.cancelCalls, isEmpty);
      expect(
        telepathy.beginCalls.single.contacts.single.nickname(),
        'Beta Contact',
        reason: 'the committed backend payload must remain the snapshot '
            'captured while frontend mutations were blocked',
      );
    });

    test(
        'removeProfile active blocks every contact and room mutation while '
        'begin is pending, then cancels on pre-commit persistence failure',
        () async {
      const storage = FlutterSecureStorage();
      final options = buildOptions();
      final seeded = await seedActiveMutations(
        storage: storage,
        options: options,
      );
      final controller = seeded.controller;
      addTearDown(controller.dispose);

      bool? pendingWhenReplacementWasCloned;
      final replacementContact = _MutableContact(
        id: 'contact-beta',
        nickname: 'Beta Replacement Contact',
        peerId: '12D3KooWContactBeta0000000000000000000000000000000',
        onClone: () {
          pendingWhenReplacementWasCloned = controller.isIdentitySwitchPending;
        },
      );
      controller.profiles['profile-beta']!.contacts[replacementContact.id()] =
          replacementContact;

      final contactNicknameBefore = seeded.contact.nickname();
      final contactVolumeBefore = seeded.contact.outputVolume();
      final roomNicknameBefore = seeded.room.nickname;
      final contactsBefore = Map<String, Contact>.from(controller.contacts);
      final roomsBefore = Map<String, Room>.from(controller.rooms);
      final storageBefore = await storage.readAll();
      final telepathy = _RecordingTelepathy()..pauseBeginIdentitySwitch();
      options.throwOnceOnSetStringKey = 'activeProfile';

      final removalFuture = controller.removeProfile(
        'profile-alpha',
        telepathy: telepathy,
      );
      await telepathy.beginIdentitySwitchEntered.future;

      expect(pendingWhenReplacementWasCloned, isTrue,
          reason: 'the Dart mutation gate must close before traversing the '
              'replacement contact map to build the backend snapshot');
      expect(controller.isIdentitySwitchPending, isTrue);
      expect(telepathy.beginCalls.single.contacts, hasLength(1));

      expectContactAndRoomMutationsBlocked(
        controller: controller,
        contact: seeded.contact,
        room: seeded.room,
      );
      await pumpEventQueue();

      expect(controller.activeProfile, 'profile-alpha');
      expect(controller.contacts, contactsBefore);
      expect(controller.rooms, roomsBefore);
      expect(seeded.contact.nickname(), contactNicknameBefore);
      expect(seeded.contact.outputVolume(), contactVolumeBefore);
      expect(seeded.room.nickname, roomNicknameBefore);
      expect(await storage.readAll(), storageBefore);

      telepathy.releaseBeginIdentitySwitch();
      await expectLater(
        removalFuture,
        throwsA(
          isA<ProfileDeletionException>().having(
            (error) => error.phase,
            'phase',
            ProfileDeletionPhase.activeIdPersist,
          ),
        ),
      );

      expect(controller.activeProfile, 'profile-alpha');
      expect(controller.profiles.keys,
          containsAll(<String>['profile-alpha', 'profile-beta']));
      expect(controller.isIdentitySwitchPending, isFalse);
      expect(telepathy.commitCalls, isEmpty);
      expect(telepathy.cancelCalls, hasLength(1));
      expect(
        telepathy.beginCalls.single.contacts.single.nickname(),
        'Beta Replacement Contact',
      );
    });

    test(
        'switchActiveProfile: when active-profile persistence fails AFTER '
        'begin_identity_switch succeeds, the controller cancels before '
        'committing Rust and restores the frontend to its prior state',
        () async {
      const storage = FlutterSecureStorage();
      final options = buildOptions();
      final controller = await seedTwo(storage: storage, options: options);
      final telepathy = _RecordingTelepathy();
      options.throwOnSetString = true;

      await expectLater(
        controller.switchActiveProfile('profile-beta', telepathy: telepathy),
        throwsA(isA<Object>()),
      );

      expect(telepathy.beginCalls, hasLength(1),
          reason: 'the transaction must reach begin so the snapshot is '
              'captured before persistence is attempted');
      expect(telepathy.commitCalls, isEmpty,
          reason: 'commit must NOT run when frontend persistence failed; '
              'per the two-phase contract the controller cancels instead');
      expect(telepathy.cancelCalls, hasLength(1),
          reason: 'a frontend persistence failure between begin and commit '
              'must cancel so the backend releases the gate without '
              'mutating the signing identity');
      expect(controller.activeProfile, 'profile-alpha',
          reason: 'the frontend active profile must be restored to its '
              'pre-switch value so it matches the identity Rust never '
              'mutated');
      expect(controller.isIdentitySwitchPending, isFalse,
          reason: 'the transaction flag must clear on every exit path so '
              'subsequent mutations are not blocked forever');
    });

    test(
        'switchActiveProfile: when commit_identity_switch fails, the '
        'frontend rolls back its persisted active profile so the UI matches '
        'the identity Rust restored internally', () async {
      const storage = FlutterSecureStorage();
      final options = buildOptions();
      final controller = await seedTwo(storage: storage, options: options);
      final telepathy = _RecordingTelepathy()
        ..commitException = Exception('manager restart failed');

      await expectLater(
        controller.switchActiveProfile('profile-beta', telepathy: telepathy),
        throwsA(isA<Object>()),
      );

      expect(telepathy.beginCalls, hasLength(1));
      expect(telepathy.commitCalls, hasLength(1),
          reason: 'commit must be attempted; the controller cannot know it '
              'will fail until it tries');
      expect(telepathy.cancelCalls, isEmpty,
          reason: 'cancel is for pre-commit failures only; once commit has '
              'been called the backend owns the rollback');
      expect(controller.activeProfile, 'profile-alpha',
          reason: 'commit failure must restore the frontend active profile '
              'so it matches the identity Rust rolled back to');
      final persistedActive = await options.getString('activeProfile');
      expect(persistedActive, 'profile-alpha',
          reason: 'the rollback must also persist; otherwise the next '
              'startup would resurrect the failed target profile as active');
      expect(controller.isIdentitySwitchPending, isFalse);
    });

    test(
        'removeProfile (active): when active-profile persistence fails AFTER '
        'begin_identity_switch succeeds, the controller cancels before '
        'committing Rust, restores the active profile, and undoes any '
        'replacement it created', () async {
      const storage = FlutterSecureStorage();
      final options = buildOptions();
      final controller = await seedTwo(storage: storage, options: options);
      final telepathy = _RecordingTelepathy();
      options.throwOnSetString = true;

      final profilesBefore = Map<String, Profile>.from(controller.profiles);

      await expectLater(
        controller.removeProfile('profile-alpha', telepathy: telepathy),
        throwsA(isA<ProfileDeletionException>()),
      );

      expect(telepathy.beginCalls, hasLength(1),
          reason: 'the transaction must reach begin so the snapshot is '
              'captured before persistence is attempted');
      expect(telepathy.commitCalls, isEmpty,
          reason: 'commit must NOT run when frontend persistence failed; '
              'the controller cancels instead');
      expect(telepathy.cancelCalls, hasLength(1),
          reason: 'a frontend persistence failure between begin and commit '
              'must cancel so the backend releases the gate without '
              'mutating the signing identity');
      expect(controller.activeProfile, 'profile-alpha',
          reason: 'the active profile must stay on the value Rust still '
              'knows about');
      expect(controller.profiles.keys.toSet(), profilesBefore.keys.toSet(),
          reason: 'no profile should be removed when the transaction '
              'failed before commit; the deleted profile and any '
              'replacement creation must be rolled back');
      expect(controller.isIdentitySwitchPending, isFalse);
    });

    test(
        'removeProfile (active): when commit_identity_switch fails, the '
        'frontend restores the prior active profile and any replacement '
        'created solely for the transaction is undone', () async {
      const storage = FlutterSecureStorage();
      final options = buildOptions();
      final controller = await seedTwo(storage: storage, options: options);
      final telepathy = _RecordingTelepathy()
        ..commitException = Exception('manager restart failed');

      final profilesBefore = Map<String, Profile>.from(controller.profiles);

      await expectLater(
        controller.removeProfile('profile-alpha', telepathy: telepathy),
        throwsA(isA<ProfileDeletionException>()),
      );

      expect(telepathy.beginCalls, hasLength(1));
      expect(telepathy.commitCalls, hasLength(1));
      expect(telepathy.cancelCalls, isEmpty);
      expect(controller.activeProfile, 'profile-alpha',
          reason: 'commit failure must restore the frontend active profile '
              'so it matches the identity Rust rolled back to');
      expect(controller.profiles.keys.toSet(), profilesBefore.keys.toSet(),
          reason: 'the deleted profile must be restored when the '
              'transaction fails; otherwise the frontend would lose a '
              'profile whose identity Rust is still running');
      expect(controller.profiles, contains('profile-alpha'));
      expect(controller.profiles, contains('profile-beta'));
      expect(controller.isIdentitySwitchPending, isFalse);
    });

    test(
        'removeProfile (active): when commit_identity_switch fails with a '
        'pre-existing replacement, the restored profilesV2 index is '
        'durable so a fresh controller still sees the original profile '
        'as active', () async {
      // Regression: the commit-failure path previously restored the
      // `profiles` map only in memory. The earlier indexWrite step
      // had already persisted `profilesV2` without the deleted id, and
      // nothing in the catch block re-persisted the restored map, so
      // the next startup loaded a controller whose index disagreed
      // with the still-intact secure-storage records for the deleted
      // id. The original profile silently disappeared. The fix
      // durably re-persists BOTH the restored index and the restored
      // active id before clearing the deletion intent; a fresh
      // controller initialized from the same prefs must therefore
      // observe the original profile as active.
      const storage = FlutterSecureStorage();
      final options = buildOptions();
      final controller = await seedTwo(storage: storage, options: options);
      final telepathy = _RecordingTelepathy()
        ..commitException = Exception('manager restart failed');

      await expectLater(
        controller.removeProfile('profile-alpha', telepathy: telepathy),
        throwsA(isA<ProfileDeletionException>()),
      );

      // Persisted index must still contain the original (deleted-then-
      // restored) id. Earlier path left the post-deletion index in
      // place because nothing re-persisted the restored map; the fix
      // re-persists it before clearing the intent.
      final persistedIndex =
          await options.getStringList('profilesV2') ?? const <String>[];
      expect(persistedIndex, contains('profile-alpha'),
          reason: 'the restored profiles map must be durably '
              're-persisted so the next startup does not load a '
              'half-deleted index that lost the original profile');
      expect(persistedIndex, contains('profile-beta'),
          reason: 'the pre-existing replacement must remain in the '
              'index after the rollback');

      // Persisted active id must be restored to the original so the
      // next startup does not resurrect the replacement the backend
      // never finished committing.
      final persistedActive = await options.getString('activeProfile');
      expect(persistedActive, 'profile-alpha',
          reason: 'the restored active id must be durable so the next '
              'startup matches the identity Rust rolled back to');

      // The deletion tombstone must be cleared once the restored index
      // is durable; otherwise startup would redrive the cleanup
      // against an id that is back in the index.
      final tombstones =
          await options.getStringList('profileDeletionTombstones') ??
              const <String>[];
      expect(tombstones, isNot(contains('profile-alpha')),
          reason: 'once the restored index is durable, the tombstone '
              'must be cleared so startup does not redrive cleanup '
              'against a profile that should exist');

      // No rollback intent should be retained when the restored
      // writes are durable.
      final rollbackIntents =
          await options.getStringList('profileRollbackIntents') ??
              const <String>[];
      expect(rollbackIntents, isNot(contains('profile-alpha')),
          reason: 'a successful durable rollback must not leave a '
              'rollback-intent marker behind');

      // A fresh controller initialized from the same persisted state
      // must select the original profile as active. This is the
      // user-observable invariant: the failed deletion leaves the
      // app in the same state the user started in.
      final controller2 = ProfilesController(
        storage: storage,
        options: options,
      );
      await controller2.init(const <String>[]);
      expect(controller2.activeProfile, 'profile-alpha',
          reason: 'the restored active id must be durable so the next '
              'startup resurrects the original profile as active');
      expect(controller2.profiles, contains('profile-alpha'),
          reason: 'the original profile must still load on the next '
              'startup because its storage records were never deleted '
              'and its id is back in the persisted index');
      expect(controller2.profiles, contains('profile-beta'),
          reason: 'the pre-existing replacement must still load on '
              'the next startup');
    });

    test(
        'removeProfile (non-active): does not invoke the two-phase '
        'transaction and rolls back the profile map on storage failure',
        () async {
      const storage = FlutterSecureStorage();
      final options = buildOptions();
      final controller = await seedTwo(storage: storage, options: options);
      final telepathy = _RecordingTelepathy();
      options.throwOnSetStringListKey = 'profilesV2';

      await expectLater(
        controller.removeProfile('profile-gamma', telepathy: telepathy),
        throwsA(isA<ProfileDeletionException>()),
      );

      expect(telepathy.beginCalls, isEmpty,
          reason: 'non-active deletion does not touch the identity; the '
              'transaction must not fire');
      expect(telepathy.commitCalls, isEmpty);
      expect(controller.profiles, contains('profile-gamma'),
          reason: 'a failed non-active delete must restore the profile so '
              'the index matches the still-present storage entries');
      expect(controller.isIdentitySwitchPending, isFalse);
    });
  });

  group('ProfilesController._loadProfile key-length validation', () {
    test(
        'rejects a profile whose persisted keypair is not 32 bytes so it '
        'cannot become a switch target and wedge the call slot', () async {
      // Regression for the wedge bug: previously _loadProfile accepted any
      // base64-decoded blob and stored it as the profile keypair. When the
      // user then tried to switch to that profile, the (old) commit API
      // hit its key-length validation AFTER begin had already reserved
      // the call slot, leaving the slot permanently held because the
      // commit-time validation failure bypassed the snapshot cleanup.
      // With validation at load time, the malformed profile is rejected
      // before it can ever become a switch target.
      const storage = FlutterSecureStorage();
      final options = SharedPreferencesAsync();
      final controller = ProfilesController(storage: storage, options: options);

      // Seed a malformed profile whose keypair decodes to 16 bytes.
      await storage.write(
          key: 'bad-keypair', value: base64Encode(List<int>.filled(16, 1)));
      await storage.write(
          key: 'bad-peerId',
          value: '12D3KooWBadPeerId0000000000000000000000000000000');
      await storage.write(key: 'bad-nickname', value: 'Bad Key Profile');

      // Seed a valid profile alongside so init() does not fall through to
      // the default-creation branch (which would call generateKeys() and
      // require the Rust runtime, unavailable in this unit-test scope).
      await storage.write(
          key: 'valid-keypair', value: base64Encode(List<int>.filled(32, 9)));
      await storage.write(
          key: 'valid-peerId',
          value: '12D3KooWValidPeerId0000000000000000000000000000000');
      await storage.write(key: 'valid-nickname', value: 'Valid Profile');
      await options.setStringList('profilesV2', const <String>['bad', 'valid']);

      await controller.init(const <String>[]);

      expect(controller.profiles, isNot(contains('bad')),
          reason: 'a profile with a non-32-byte keypair must be rejected '
              'at load time so it cannot be selected as a switch target');
      expect(controller.profiles, contains('valid'),
          reason: 'a sibling valid profile must still load so the user can '
              'use the app after a malformed-profile rejection');
    });

    test(
        'rejects a profile whose keypair decodes to 33 bytes (off-by-one '
        'regression guard)', () async {
      const storage = FlutterSecureStorage();
      final options = SharedPreferencesAsync();
      final controller = ProfilesController(storage: storage, options: options);

      await storage.write(
          key: 'off-by-one-keypair',
          value: base64Encode(List<int>.filled(33, 1)));
      await storage.write(
          key: 'off-by-one-peerId',
          value: '12D3KooWOffByOne00000000000000000000000000000000');
      await storage.write(
          key: 'valid-keypair', value: base64Encode(List<int>.filled(32, 9)));
      await storage.write(
          key: 'valid-peerId',
          value: '12D3KooWValidPeerId0000000000000000000000000000000');
      await options
          .setStringList('profilesV2', const <String>['off-by-one', 'valid']);

      await controller.init(const <String>[]);

      expect(controller.profiles, isNot(contains('off-by-one')));
      expect(controller.profiles, contains('valid'));
    });

    test('accepts a profile with a valid 32-byte keypair', () async {
      const storage = FlutterSecureStorage();
      final options = SharedPreferencesAsync();
      final controller = ProfilesController(storage: storage, options: options);

      await storage.write(
          key: 'valid-keypair', value: base64Encode(List<int>.filled(32, 7)));
      await storage.write(
          key: 'valid-peerId',
          value: '12D3KooWValidPeerId0000000000000000000000000000000');
      await storage.write(key: 'valid-nickname', value: 'Valid Profile');
      await options.setStringList('profilesV2', const <String>['valid']);

      await controller.init(const <String>[]);

      expect(controller.profiles, contains('valid'));
      expect(controller.profiles['valid']!.nickname, 'Valid Profile');
    });
  });

  group('ProfilesController profile journal fault injection + tombstones', () {
    _ThrowingSharedPreferences buildOptions() => _ThrowingSharedPreferences();
    final rustApi = _RecordingRustApi();

    setUpAll(() {
      RustLib.initMock(api: rustApi);
    });

    setUp(() {
      rustApi.generateKeysCalls = 0;
    });

    tearDownAll(RustLib.dispose);

    SeedProfile realisticSeed(String id, {String nickname = 'Profile'}) {
      final seed = id.hashCode & 0xFF;
      return (
        id: id,
        nickname: nickname,
        peerId: '12D3KooW${id}PeerId00000000000000000000000000000',
        keypair: List<int>.filled(32, seed),
      );
    }

    String rollbackRecord(
      String deletedProfileId,
      String previousActiveProfileId, {
      String phase = 'prepared',
    }) {
      return jsonEncode(<String, Object>{
        'version': 1,
        'deletedProfileId': deletedProfileId,
        'previousActiveProfileId': previousActiveProfileId,
        'phase': phase,
      });
    }

    test(
        'active delete: prepared rollback journal recovers when all rollback '
        'preference writes fail together', () async {
      const storage = FlutterSecureStorage();
      final options = buildOptions();
      final controller = await seedController(
        storage: storage,
        options: options,
        profiles: <SeedProfile>[
          realisticSeed('profile-alpha', nickname: 'Alpha'),
          realisticSeed('profile-beta', nickname: 'Beta'),
        ],
        activeProfileId: 'profile-alpha',
      );
      final telepathy = _RecordingTelepathy()
        ..commitException = Exception('manager restart failed')
        ..onCommit = () async {
          options.throwOnSetString = true;
          options.throwOnSetStringList = true;
        };

      await expectLater(
        controller.removeProfile('profile-alpha', telepathy: telepathy),
        throwsA(
          isA<ProfileDeletionException>().having(
            (error) => error.phase,
            'phase',
            ProfileDeletionPhase.commit,
          ),
        ),
      );

      expect(controller.activeProfile, 'profile-alpha');
      expect(controller.profiles.keys,
          containsAll(<String>['profile-alpha', 'profile-beta']));
      expect(await options.getStringList('profilesV2'),
          orderedEquals(<String>['profile-beta']));
      expect(await options.getString('activeProfile'), 'profile-beta');
      expect(
        await options.getStringList('profileDeletionTombstones'),
        contains('profile-alpha'),
      );
      final journal =
          await options.getStringList('profileDeletionRollbackJournal') ??
              const <String>[];
      expect(journal, hasLength(1));
      expect(
        jsonDecode(journal.single),
        <String, Object>{
          'version': 1,
          'deletedProfileId': 'profile-alpha',
          'previousActiveProfileId': 'profile-alpha',
          'phase': 'prepared',
        },
      );
      expect(await storage.read(key: 'profile-alpha-keypair'), isNotNull);

      options
        ..throwOnSetString = false
        ..throwOnSetStringList = false;
      final restarted = ProfilesController(storage: storage, options: options);
      await restarted.init(const <String>[]);

      expect(restarted.profiles.keys,
          containsAll(<String>['profile-alpha', 'profile-beta']));
      expect(restarted.activeProfile, 'profile-alpha');
      expect(await storage.read(key: 'profile-alpha-keypair'), isNotNull);
      expect(await options.getStringList('profileDeletionTombstones'), isEmpty);
      expect(
        await options.getStringList('profileDeletionRollbackJournal'),
        isEmpty,
      );
    });

    test(
        'active delete: index-restored journal recovers when only active-ID '
        'rollback persistence fails', () async {
      const storage = FlutterSecureStorage();
      final options = buildOptions();
      final controller = await seedController(
        storage: storage,
        options: options,
        profiles: <SeedProfile>[
          realisticSeed('profile-alpha', nickname: 'Alpha'),
          realisticSeed('profile-beta', nickname: 'Beta'),
        ],
        activeProfileId: 'profile-alpha',
      );
      final telepathy = _RecordingTelepathy()
        ..commitException = Exception('manager restart failed')
        ..onCommit = () async {
          options.throwOnSetStringKey = 'activeProfile';
        };

      await expectLater(
        controller.removeProfile('profile-alpha', telepathy: telepathy),
        throwsA(
          isA<ProfileDeletionException>().having(
            (error) => error.phase,
            'phase',
            ProfileDeletionPhase.commit,
          ),
        ),
      );

      expect(await options.getStringList('profilesV2'),
          containsAll(<String>['profile-alpha', 'profile-beta']));
      expect(await options.getString('activeProfile'), 'profile-beta');
      final journal =
          await options.getStringList('profileDeletionRollbackJournal') ??
              const <String>[];
      expect(journal, hasLength(1));
      expect(
        (jsonDecode(journal.single) as Map<String, dynamic>)['phase'],
        'indexRestored',
      );
      expect(
        await options.getStringList('profileDeletionTombstones'),
        contains('profile-alpha'),
      );

      options.throwOnSetStringKey = null;
      final restarted = ProfilesController(storage: storage, options: options);
      await restarted.init(const <String>[]);

      expect(restarted.profiles.keys,
          containsAll(<String>['profile-alpha', 'profile-beta']));
      expect(restarted.activeProfile, 'profile-alpha');
      expect(await storage.read(key: 'profile-alpha-keypair'), isNotNull);
      expect(await options.getStringList('profileDeletionTombstones'), isEmpty);
      expect(
        await options.getStringList('profileDeletionRollbackJournal'),
        isEmpty,
      );
    });

    test(
        'startup recovery restores two pending rollback records with one '
        'cumulative profile-index write', () async {
      const storage = FlutterSecureStorage();
      final options = buildOptions();
      final seeded = await seedController(
        storage: storage,
        options: options,
        profiles: <SeedProfile>[
          realisticSeed('profile-alpha', nickname: 'Alpha'),
          realisticSeed('profile-beta', nickname: 'Beta'),
          realisticSeed('profile-gamma', nickname: 'Gamma'),
        ],
        activeProfileId: 'profile-beta',
      );
      seeded.dispose();
      await options.setStringList(
        'profilesV2',
        const <String>['profile-beta'],
      );
      await options.setString('activeProfile', 'profile-beta');
      await options.setStringList(
        'profileDeletionTombstones',
        const <String>['profile-alpha', 'profile-gamma'],
      );
      await options.setStringList(
        'profileDeletionRollbackJournal',
        <String>[
          rollbackRecord('profile-alpha', 'profile-alpha'),
          rollbackRecord('profile-gamma', 'profile-gamma'),
        ],
      );
      options.successfulWrites.clear();

      final recovered = ProfilesController(storage: storage, options: options);
      await recovered.init(const <String>[]);

      final indexWrites = options.successfulWrites
          .where((write) => write.key == 'profilesV2')
          .toList();
      expect(indexWrites, hasLength(1));
      expect(
        indexWrites.single.value,
        containsAll(<String>['profile-alpha', 'profile-beta', 'profile-gamma']),
      );
      expect(
          recovered.profiles.keys,
          containsAll(
              <String>['profile-alpha', 'profile-beta', 'profile-gamma']));
      expect(recovered.activeProfile, 'profile-gamma');
      expect(await storage.read(key: 'profile-alpha-keypair'), isNotNull);
      expect(await storage.read(key: 'profile-gamma-keypair'), isNotNull);

      for (final id in <String>['profile-alpha', 'profile-gamma']) {
        final activeWriteIndex = options.successfulWrites.indexWhere(
          (write) => write.key == 'activeProfile' && write.value == id,
        );
        final tombstoneRemovalIndex = options.successfulWrites.indexWhere(
          (write) =>
              write.key == 'profileDeletionTombstones' &&
              !(write.value as List<String>).contains(id),
        );
        final journalRemovalIndex = options.successfulWrites.indexWhere(
          (write) =>
              write.key == 'profileDeletionRollbackJournal' &&
              !(write.value as List<String>).any(
                (encoded) =>
                    (jsonDecode(encoded)
                        as Map<String, dynamic>)['deletedProfileId'] ==
                    id,
              ),
        );
        expect(activeWriteIndex, isNonNegative);
        expect(tombstoneRemovalIndex, greaterThan(activeWriteIndex));
        expect(journalRemovalIndex, greaterThan(activeWriteIndex));
      }
      expect(await options.getStringList('profileDeletionTombstones'), isEmpty);
      expect(
        await options.getStringList('profileDeletionRollbackJournal'),
        isEmpty,
      );
    });

    test(
        'startup recovery stops after the first active-ID write failure so '
        'later rollback records retain their original order', () async {
      const storage = FlutterSecureStorage();
      final options = buildOptions();
      final seeded = await seedController(
        storage: storage,
        options: options,
        profiles: <SeedProfile>[
          realisticSeed('profile-alpha', nickname: 'Alpha'),
          realisticSeed('profile-beta', nickname: 'Beta'),
          realisticSeed('profile-gamma', nickname: 'Gamma'),
        ],
        activeProfileId: 'profile-beta',
      );
      seeded.dispose();
      await options.setStringList(
        'profilesV2',
        const <String>['profile-beta'],
      );
      await options.setString('activeProfile', 'profile-beta');
      await options.setStringList(
        'profileDeletionTombstones',
        const <String>['profile-alpha', 'profile-gamma'],
      );
      await options.setStringList(
        'profileDeletionRollbackJournal',
        <String>[
          rollbackRecord('profile-alpha', 'profile-alpha'),
          rollbackRecord('profile-gamma', 'profile-gamma'),
        ],
      );
      options
        ..successfulWrites.clear()
        ..throwOnceOnSetStringKey = 'activeProfile';

      final interrupted =
          ProfilesController(storage: storage, options: options);
      await interrupted.init(const <String>[]);

      expect(
        await options.getStringList('profilesV2'),
        containsAll(<String>['profile-alpha', 'profile-beta', 'profile-gamma']),
        reason: 'the cumulative index restoration must remain durable even '
            'when per-record active restoration stops',
      );
      expect(interrupted.activeProfile, 'profile-beta',
          reason: 'a later rollback record must not become active after an '
              'earlier record failed to restore its active id');
      final interruptedJournal =
          await options.getStringList('profileDeletionRollbackJournal') ??
              const <String>[];
      expect(
        interruptedJournal.map(
          (encoded) =>
              (jsonDecode(encoded) as Map<String, dynamic>)['deletedProfileId'],
        ),
        orderedEquals(<String>['profile-alpha', 'profile-gamma']),
        reason: 'both records must remain protected in original journal order',
      );
      expect(
        (jsonDecode(interruptedJournal.first) as Map<String, dynamic>)['phase'],
        'indexRestored',
      );
      expect(
        (jsonDecode(interruptedJournal.last) as Map<String, dynamic>)['phase'],
        'prepared',
        reason: 'later records must not advance after an earlier recovery '
            'failure',
      );
      expect(
        await options.getStringList('profileDeletionTombstones'),
        orderedEquals(<String>['profile-alpha', 'profile-gamma']),
      );
      expect(
        options.successfulWrites.where(
          (write) =>
              write.key == 'activeProfile' && write.value == 'profile-gamma',
        ),
        isEmpty,
        reason: 'the later active restoration must not run out of order',
      );

      interrupted.dispose();
      final recovered = ProfilesController(storage: storage, options: options);
      await recovered.init(const <String>[]);

      expect(recovered.activeProfile, 'profile-gamma',
          reason: 'the next startup must replay both records in their original '
              'order, ending on the later prior-active id');
      expect(
        await options.getStringList('profileDeletionRollbackJournal'),
        isEmpty,
      );
      expect(await options.getStringList('profileDeletionTombstones'), isEmpty);
    });

    test(
        'sole active delete: real controller path creates a durable replacement '
        'and removes the original without resurrection', () async {
      const storage = FlutterSecureStorage();
      final options = buildOptions();
      final controller = await seedController(
        storage: storage,
        options: options,
        profiles: <SeedProfile>[
          realisticSeed('profile-alpha', nickname: 'Alpha'),
        ],
        activeProfileId: 'profile-alpha',
      );
      addTearDown(controller.dispose);
      final telepathy = _RecordingTelepathy();
      late final ({
        String activeProfile,
        List<String>? index,
        String? persistedActive
      }) stateAtCommit;
      telepathy.onCommit = () async {
        stateAtCommit = (
          activeProfile: controller.activeProfile,
          index: await options.getStringList('profilesV2'),
          persistedActive: await options.getString('activeProfile'),
        );
      };

      await controller.removeProfile('profile-alpha', telepathy: telepathy);

      final replacementId = controller.profiles.keys.single;
      const generatedPeerId =
          '12D3KooWGeneratedPeerId000000000000000000000000000000';
      final generatedKeypair = List<int>.filled(32, 7);

      expect(replacementId, isNot('profile-alpha'));
      expect(rustApi.generateKeysCalls, 1,
          reason: 'the sole active profile must create exactly one replacement '
              'through the deterministic Rust bridge mock');
      expect(controller.activeProfile, replacementId);
      expect(controller.profiles, isNot(contains('profile-alpha')));
      expect(controller.profiles[replacementId]!.nickname, 'Default');
      expect(controller.profiles[replacementId]!.peerId, generatedPeerId);
      expect(controller.profiles[replacementId]!.keypair, generatedKeypair);

      expect(telepathy.beginCalls, hasLength(1));
      expect(telepathy.beginCalls.single.key, generatedKeypair,
          reason: 'the transaction must begin with the replacement key');
      expect(telepathy.beginCalls.single.contacts, isEmpty,
          reason: 'a newly created replacement has no contacts to switch');
      expect(telepathy.commitCalls, hasLength(1),
          reason: 'the replacement identity must commit after persistence');
      expect(telepathy.cancelCalls, isEmpty);
      expect(stateAtCommit.activeProfile, replacementId,
          reason: 'the in-memory active profile must switch before commit');
      expect(stateAtCommit.index, orderedEquals(<String>[replacementId]),
          reason: 'the durable index must exclude the deleted profile before '
              'commit');
      expect(stateAtCommit.persistedActive, replacementId,
          reason: 'the durable active profile must switch before commit');

      expect(await options.getStringList('profilesV2'),
          orderedEquals(<String>[replacementId]),
          reason: 'the durable index must exclude the deleted profile');
      expect(await options.getString('activeProfile'), replacementId,
          reason: 'the replacement must be durably active before commit');
      expect(await storage.read(key: '$replacementId-keypair'),
          base64Encode(generatedKeypair));
      expect(await storage.read(key: '$replacementId-peerId'), generatedPeerId);
      expect(await storage.read(key: '$replacementId-nickname'), 'Default');
      expect(await storage.read(key: '$replacementId-contacts'), '{}');
      expect(await storage.read(key: '$replacementId-rooms'), '{}');

      for (final suffix in <String>[
        'keypair',
        'peerId',
        'contacts',
        'rooms',
        'nickname',
      ]) {
        expect(await storage.read(key: 'profile-alpha-$suffix'), isNull,
            reason: 'post-commit cleanup must remove the original $suffix '
                'record');
      }
      expect(await options.getStringList('profileDeletionTombstones'), isEmpty,
          reason: 'the deletion journal must clear after secure-storage '
              'cleanup succeeds');

      final restarted = ProfilesController(storage: storage, options: options);
      addTearDown(restarted.dispose);
      await restarted.init(const <String>[]);

      expect(restarted.profiles.keys, orderedEquals(<String>[replacementId]),
          reason: 'a fresh controller must not resurrect the deleted profile');
      expect(restarted.activeProfile, replacementId);
      expect(restarted.profiles[replacementId]!.peerId, generatedPeerId);
      expect(restarted.profiles[replacementId]!.keypair, generatedKeypair);
    });

    test(
        'createProfile: deletion-journal write failure aborts before key '
        'generation and leaves no profile state', () async {
      const storage = FlutterSecureStorage();
      final options = buildOptions()
        ..throwOnSetStringListKey = 'profileDeletionTombstones';
      final controller = ProfilesController(
        storage: storage,
        options: options,
      );

      await expectLater(
        controller.createProfile('Journal Failure'),
        throwsA(isA<Object>()),
      );

      expect(rustApi.generateKeysCalls, 0,
          reason: 'a durable cleanup journal must exist before generating a '
              'private key that could otherwise become orphaned');
      expect(controller.profiles, isEmpty,
          reason: 'failed journal persistence must not publish a profile in '
              'memory');
      expect(await options.getStringList('profilesV2'), isNull,
          reason: 'failed journal persistence must not create a profile '
              'index entry');
      expect(await options.getStringList('profileDeletionTombstones'), isNull,
          reason: 'the injected write failed, so no partial journal state '
              'should be visible');
      expect(await storage.readAll(), isEmpty,
          reason: 'no secure-storage record may be written before the '
              'cleanup journal is durable');
    });

    test(
        'sole active delete: unavailable deletion journal aborts before '
        'replacement key generation and preserves every layer', () async {
      const storage = FlutterSecureStorage();
      final options = buildOptions();
      final controller = await seedController(
        storage: storage,
        options: options,
        profiles: <SeedProfile>[
          realisticSeed('profile-alpha', nickname: 'Alpha'),
        ],
        activeProfileId: 'profile-alpha',
      );
      final storageBefore = await storage.readAll();
      final originalProfile = controller.profiles['profile-alpha'];
      final telepathy = _RecordingTelepathy();
      options.throwOnSetStringListKey = 'profileDeletionTombstones';

      await expectLater(
        controller.removeProfile('profile-alpha', telepathy: telepathy),
        throwsA(
          isA<ProfileDeletionException>().having(
            (error) => error.phase,
            'phase',
            ProfileDeletionPhase.tombstoneWrite,
          ),
        ),
      );

      expect(rustApi.generateKeysCalls, 0,
          reason: 'journal availability must be established before creating '
              'a replacement identity for the sole active profile');
      expect(
        controller.profiles.keys,
        orderedEquals(<String>['profile-alpha']),
      );
      expect(controller.profiles['profile-alpha'], same(originalProfile),
          reason: 'the original in-memory profile must remain untouched');
      expect(controller.activeProfile, 'profile-alpha');
      expect(controller.isIdentitySwitchPending, isFalse);
      expect(await options.getStringList('profilesV2'),
          orderedEquals(<String>['profile-alpha']),
          reason: 'the persisted profile index must remain unchanged');
      expect(await options.getString('activeProfile'), 'profile-alpha',
          reason: 'the persisted active identity must remain unchanged');
      expect(await storage.readAll(), storageBefore,
          reason: 'neither replacement nor original secure-storage records '
              'may change when the journal is unavailable');
      expect(telepathy.beginCalls, isEmpty,
          reason: 'the backend gate must not be acquired before the journal '
              'precondition succeeds');
      expect(telepathy.commitCalls, isEmpty);
      expect(telepathy.cancelCalls, isEmpty);
    });

    test(
        'sole active delete: failed replacement index rollback preserves the '
        'replacement as a valid profile', () async {
      const storage = FlutterSecureStorage();
      final options = buildOptions();
      final controller = await seedController(
        storage: storage,
        options: options,
        profiles: <SeedProfile>[
          realisticSeed('profile-alpha', nickname: 'Alpha'),
        ],
        activeProfileId: 'profile-alpha',
      );
      String? replacementId;
      final telepathy = _RecordingTelepathy()
        ..onBegin = () {
          replacementId = controller.profiles.keys.singleWhere(
            (id) => id != 'profile-alpha',
          );
          options.throwOnSetStringListKey = 'profilesV2';
        };

      await expectLater(
        controller.removeProfile('profile-alpha', telepathy: telepathy),
        throwsA(
          isA<ProfileDeletionException>().having(
            (error) => error.phase,
            'phase',
            ProfileDeletionPhase.indexWrite,
          ),
        ),
      );

      final id = replacementId!;
      expect(controller.activeProfile, 'profile-alpha');
      expect(controller.profiles, contains(id),
          reason: 'a replacement that could not be durably excluded remains '
              'a valid in-memory profile');
      expect(await options.getStringList('profilesV2'), contains(id),
          reason: 'the successful creation write remains authoritative when '
              'the rollback index write fails');
      expect(
        await storage.read(key: '$id-keypair'),
        base64Encode(List<int>.filled(32, 7)),
        reason: 'rollback must not delete the keypair for an indexed profile',
      );
      expect(await storage.read(key: '$id-peerId'), isNotNull);
      expect(await storage.read(key: '$id-nickname'), isNotNull);
      expect(
        await options.getStringList('profileDeletionTombstones'),
        contains(id),
        reason: 'the durable rollback journal remains until startup '
            'reconciles the retained indexed profile',
      );
      expect(telepathy.beginCalls, hasLength(1));
      expect(telepathy.commitCalls, isEmpty);
      expect(telepathy.cancelCalls, hasLength(1),
          reason: 'the failed pre-commit index write must release the backend '
              'switch slot without changing identity');

      options.throwOnSetStringListKey = null;
      final restarted = ProfilesController(storage: storage, options: options);
      await restarted.init(const <String>[]);
      expect(restarted.profiles, contains(id),
          reason: 'a fresh controller must load the retained replacement '
              'from its durable index and storage');
      expect(restarted.profiles[id]!.keypair, List<int>.filled(32, 7));
    });

    test(
        'non-active delete: profile-index persistence failure restores the '
        'profile so the index matches the still-present storage records',
        () async {
      const storage = FlutterSecureStorage();
      final options = buildOptions();
      final controller = await seedController(
        storage: storage,
        options: options,
        profiles: <SeedProfile>[
          realisticSeed('profile-alpha'),
          realisticSeed('profile-gamma'),
        ],
        activeProfileId: 'profile-alpha',
      );

      // Stage the failure for the next `setStringList('profilesV2', ...)`
      // call (used by _persistProfileIds during the non-active delete).
      // The tombstone write (a different key) must still succeed so the
      // test exercises the rollback path that clears the tombstone.
      options.throwOnSetStringListKey = 'profilesV2';

      await expectLater(
        controller.removeProfile(
          'profile-gamma',
          telepathy: _RecordingTelepathy(),
        ),
        throwsA(isA<ProfileDeletionException>()),
      );

      expect(controller.profiles, contains('profile-gamma'),
          reason: 'failed index persistence must roll back the in-memory '
              'removal so the next startup does not resurrect a half-deleted '
              'profile whose private key still lives in secure storage');
      expect(controller.activeProfile, 'profile-alpha');
      final tombstones =
          await options.getStringList('profileDeletionTombstones');
      expect(tombstones, isNot(contains('profile-gamma')),
          reason: 'failed index persistence must clear the tombstone it '
              'wrote before the failed write so startup does not redrive '
              'the cleanup against an id that is still in the index');
    });

    test(
        'non-active delete: a profile-index write that commits then reports '
        'failure preserves its prepared rollback record and storage', () async {
      const storage = FlutterSecureStorage();
      final options = buildOptions();
      final controller = await seedController(
        storage: storage,
        options: options,
        profiles: <SeedProfile>[
          realisticSeed('profile-alpha'),
          realisticSeed('profile-gamma'),
        ],
        activeProfileId: 'profile-alpha',
      );
      final keypairBefore = await storage.read(key: 'profile-gamma-keypair');
      options.throwAfterSetStringListKey = 'profilesV2';

      await expectLater(
        controller.removeProfile(
          'profile-gamma',
          telepathy: _RecordingTelepathy(),
        ),
        throwsA(
          isA<ProfileDeletionException>().having(
            (error) => error.phase,
            'phase',
            ProfileDeletionPhase.indexWrite,
          ),
        ),
      );

      expect(controller.profiles, contains('profile-gamma'));
      expect(controller.activeProfile, 'profile-alpha');
      expect(await storage.read(key: 'profile-gamma-keypair'), keypairBefore,
          reason: 'an ambiguous index write must never authorize secure '
              'storage deletion');
      expect(
        await options.getStringList('profileDeletionTombstones'),
        contains('profile-gamma'),
        reason: 'recovery protection must survive while rollback persistence '
            'still reports failure',
      );
      final journal =
          await options.getStringList('profileDeletionRollbackJournal') ??
              const <String>[];
      expect(journal, hasLength(1));
      expect(
        jsonDecode(journal.single),
        <String, Object>{
          'version': 1,
          'deletedProfileId': 'profile-gamma',
          'previousActiveProfileId': 'profile-alpha',
          'phase': 'prepared',
        },
        reason: 'the structured write-ahead record must predate the '
            'destructive profile-index mutation',
      );
    });

    test(
        'non-active delete: startup replays a prepared rollback record before '
        'clearing its tombstone', () async {
      const storage = FlutterSecureStorage();
      final options = buildOptions();
      final seeded = await seedController(
        storage: storage,
        options: options,
        profiles: <SeedProfile>[
          realisticSeed('profile-alpha'),
          realisticSeed('profile-gamma'),
        ],
        activeProfileId: 'profile-alpha',
      );
      seeded.dispose();
      await options.setStringList(
        'profilesV2',
        const <String>['profile-alpha'],
      );
      await options.setStringList(
        'profileDeletionTombstones',
        const <String>['profile-gamma'],
      );
      await options.setStringList(
        'profileDeletionRollbackJournal',
        <String>[
          rollbackRecord('profile-gamma', 'profile-alpha'),
        ],
      );

      final recovered = ProfilesController(storage: storage, options: options);
      await recovered.init(const <String>[]);

      expect(recovered.profiles, contains('profile-gamma'),
          reason: 'the write-ahead record must restore the id excluded by the '
              'interrupted non-active deletion');
      expect(recovered.activeProfile, 'profile-alpha',
          reason: 'recovery must durably preserve the active profile captured '
              'before the non-active deletion');
      expect(await storage.read(key: 'profile-gamma-keypair'), isNotNull,
          reason: 'rollback replay restores the profile instead of deleting '
              'its secure storage');
      expect(await options.getStringList('profileDeletionTombstones'), isEmpty);
      expect(
        await options.getStringList('profileDeletionRollbackJournal'),
        isEmpty,
      );
    });

    test(
        'non-active delete: secure-storage cleanup failure tombstones the id '
        'so a fresh controller redrives the cleanup on init', () async {
      // Regression for the silent-resurrection bug: previously the cleanup
      // failure was swallowed and the dialog closed silently, leaving the
      // private key on disk with no retry path. Now the failure surfaces
      // AND a tombstone records the pending cleanup so startup retries.
      final throwingStorage = _ThrowingFlutterSecureStorage();
      final options = buildOptions();
      final controller = await seedController(
        storage: throwingStorage,
        options: options,
        profiles: <SeedProfile>[
          realisticSeed('profile-alpha'),
          realisticSeed('profile-beta'),
        ],
        activeProfileId: 'profile-alpha',
      );

      // Stage the storage failure and run the delete.
      throwingStorage.throwOnDelete = true;

      await expectLater(
        controller.removeProfile(
          'profile-beta',
          telepathy: _RecordingTelepathy(),
        ),
        throwsA(isA<ProfileDeletionException>()),
      );

      expect(controller.profiles, isNot(contains('profile-beta')),
          reason: 'index persistence succeeded; only storage cleanup failed');

      final List<String>? tombstones =
          await options.getStringList('profileDeletionTombstones');
      expect(tombstones, contains('profile-beta'),
          reason: 'storage cleanup failure must record a tombstone so '
              'startup redrives the cleanup instead of silently dropping '
              'the retry');

      // Simulate the next startup: a fresh controller calls init, which
      // should redrive the tombstoned cleanup. Reset the throwing flag so
      // the retry can succeed.
      throwingStorage.throwOnDelete = false;
      final controller2 = ProfilesController(
        storage: throwingStorage,
        options: options,
      );
      await controller2.init(const <String>[]);

      final List<String>? tombstonesAfterRestart =
          await options.getStringList('profileDeletionTombstones');
      expect(tombstonesAfterRestart, isNot(contains('profile-beta')),
          reason: 'startup must redrive the cleanup and clear the tombstone '
              'once the storage delete succeeds');
      expect(await throwingStorage.read(key: 'profile-beta-keypair'), isNull,
          reason: 'startup retry must actually delete the private-key record');
    });

    test(
        'active delete: post-commit storage cleanup failure surfaces the '
        'error and tombstones the id for startup retry', () async {
      final throwingStorage = _ThrowingFlutterSecureStorage();
      final options = buildOptions();
      final controller = await seedController(
        storage: throwingStorage,
        options: options,
        profiles: <SeedProfile>[
          realisticSeed('profile-alpha'),
          realisticSeed('profile-beta'),
        ],
        activeProfileId: 'profile-alpha',
      );

      throwingStorage.throwOnDelete = true;

      await expectLater(
        controller.removeProfile(
          'profile-alpha',
          telepathy: _RecordingTelepathy(),
        ),
        throwsA(isA<ProfileDeletionException>()),
      );

      // Identity switch already happened (begin + commit succeeded); the
      // active profile must point at the replacement and the deleted id
      // must be gone from the index.
      expect(controller.activeProfile, 'profile-beta');
      expect(controller.profiles, isNot(contains('profile-alpha')));

      final List<String>? tombstones =
          await options.getStringList('profileDeletionTombstones');
      expect(tombstones, contains('profile-alpha'),
          reason: 'active profile storage cleanup failure must tombstone '
              'the id so startup retries the private-key removal');
    });

    // ---- Comment 1: fault-injection test for post-begin index failure ----
    test(
        'Comment 1 regression: active delete that fails the profile-index '
        'write AFTER begin rolls back Dart state, persisted state, and the '
        'backend target together so no layer desynchronizes', () async {
      // The new ordering puts the profile-index exclusion BEFORE
      // commitIdentitySwitch. Failing the index write at that point must
      // roll back the active id, the tombstone, AND cancel the backend
      // transaction. The replacement identity must NOT be committed; the
      // deleted id must remain in the index. No layer may move on while
      // another rolls back.
      const storage = FlutterSecureStorage();
      final options = buildOptions();
      final controller = await seedController(
        storage: storage,
        options: options,
        profiles: <SeedProfile>[
          realisticSeed('profile-alpha'),
          realisticSeed('profile-beta'),
        ],
        activeProfileId: 'profile-alpha',
      );
      final telepathy = _RecordingTelepathy();
      options.throwOnSetStringListKey = 'profilesV2';

      await expectLater(
        controller.removeProfile('profile-alpha', telepathy: telepathy),
        throwsA(
          isA<ProfileDeletionException>().having(
            (e) => e.phase,
            'phase',
            ProfileDeletionPhase.indexWrite,
          ),
        ),
      );

      // Dart state: active profile rolled back; the would-be-deleted id
      // is still in the in-memory map.
      expect(controller.activeProfile, 'profile-alpha',
          reason: 'a pre-commit rollback must restore the active profile '
              'to the value Rust still knows about');
      expect(controller.profiles, contains('profile-alpha'),
          reason: 'the index-write rollback must restore the deleted id so '
              'the frontend does not lose a profile whose identity Rust '
              'never swapped');
      expect(controller.profiles, contains('profile-beta'),
          reason: 'the pre-existing replacement candidate must remain');
      expect(controller.isIdentitySwitchPending, isFalse);

      // Persisted state: prefs agree with Dart.
      final persistedActive = await options.getString('activeProfile');
      expect(persistedActive, 'profile-alpha',
          reason: 'the rollback must persist so the next startup does not '
              'resurrect the failed target as active');
      final persistedIndex =
          await options.getStringList('profilesV2') ?? const <String>[];
      expect(persistedIndex,
          containsAll(<String>['profile-alpha', 'profile-beta']),
          reason: 'the index must still list both ids after the rollback');

      // Backend target: commit must NOT have fired; cancel must have.
      expect(telepathy.beginCalls, hasLength(1),
          reason: 'the transaction must reach begin so the slot is held '
              'across the index write');
      expect(telepathy.commitCalls, isEmpty,
          reason: 'commit must NOT run when the index write failed; the '
              'controller cancels so the backend rolls back to the '
              'previous identity');
      expect(telepathy.cancelCalls, hasLength(1),
          reason: 'the pre-commit rollback path must cancel the backend '
              'transaction to release the gate without mutating identity');

      // Tombstone journal: the rolled-back write must NOT leave a
      // tombstone behind, otherwise startup would redrive cleanup against
      // an id that is still in the index.
      final tombstones =
          await options.getStringList('profileDeletionTombstones') ??
              const <String>[];
      expect(tombstones, isNot(contains('profile-alpha')),
          reason: 'the index-write rollback must clear the tombstone it '
              'wrote before the failed write so startup does not redrive '
              'cleanup against an id that is still in the index');
    });

    // ---- Comment 2: tombstone-write-failure test ----
    test(
        'Comment 2 regression: when the deletion intent cannot be persisted '
        'the operation aborts before any destructive change', () async {
      // The durable tombstone write is the precondition for removing an
      // id from the index or deleting its storage. If the tombstone write
      // fails, the operation MUST abort: the index stays unchanged, the
      // storage stays unchanged, and (for active deletions) the backend
      // transaction is cancelled so the identity is not mutated.
      const storage = FlutterSecureStorage();
      final options = buildOptions();
      final controller = await seedController(
        storage: storage,
        options: options,
        profiles: <SeedProfile>[
          realisticSeed('profile-alpha'),
          realisticSeed('profile-beta'),
        ],
        activeProfileId: 'profile-alpha',
      );
      final telepathy = _RecordingTelepathy();
      options.throwOnSetStringListKey = 'profileDeletionTombstones';

      await expectLater(
        controller.removeProfile('profile-alpha', telepathy: telepathy),
        throwsA(
          isA<ProfileDeletionException>().having(
            (e) => e.phase,
            'phase',
            ProfileDeletionPhase.tombstoneWrite,
          ),
        ),
      );

      // Dart state: no destructive change.
      expect(controller.activeProfile, 'profile-alpha',
          reason: 'aborting the operation must leave the active profile '
              'unchanged');
      expect(controller.profiles, contains('profile-alpha'));
      expect(controller.profiles, contains('profile-beta'));
      expect(controller.isIdentitySwitchPending, isFalse);

      // Backend: begin + cancel (no commit). The slot must be released.
      expect(telepathy.beginCalls, hasLength(1),
          reason: 'begin runs before the tombstone write in the active '
              'deletion flow; a slot was held across the failure');
      expect(telepathy.commitCalls, isEmpty,
          reason: 'commit must NOT run when the tombstone write fails');
      expect(telepathy.cancelCalls, hasLength(1),
          reason: 'the controller must cancel so the backend releases the '
              'slot without mutating identity');

      // Persisted index must still list both ids.
      final persistedIndex =
          await options.getStringList('profilesV2') ?? const <String>[];
      expect(persistedIndex,
          containsAll(<String>['profile-alpha', 'profile-beta']),
          reason: 'no destructive change may persist when the intent write '
              'fails');
      // Active id pref must still point at the original.
      final persistedActive = await options.getString('activeProfile');
      expect(persistedActive, 'profile-alpha');
    });

    // ---- Comment 2: crash-boundary test ----
    test(
        'Comment 2 crash-boundary: a controller that starts up with a '
        'post-commit tombstone (crash between commit and storage cleanup) '
        'redrives the cleanup, clears the tombstone, and selects the '
        'replacement as the active profile', () async {
      // Simulate the crash window: tombstone was written, the id was
      // removed from the index, the active id was switched to the
      // replacement, commit succeeded on the backend, but the storage
      // cleanup did not finish. A fresh controller must redrive the
      // cleanup and select the replacement.
      final throwingStorage = _ThrowingFlutterSecureStorage();
      final options = buildOptions();

      // Seed the post-crash state directly into prefs + storage:
      // - 'profile-alpha' was the deleted id; its storage records remain.
      // - 'profile-beta' is the replacement; its storage + index entry
      //   remain.
      // - 'activeProfile' = 'profile-beta' (commit succeeded).
      // - tombstone list contains 'profile-alpha'.
      await throwingStorage.write(
        key: 'profile-alpha-keypair',
        value: base64Encode(List<int>.filled(32, 9)),
      );
      await throwingStorage.write(
        key: 'profile-alpha-peerId',
        value: '12D3KooWAlphaPeerId0000000000000000000000000000000',
      );
      await throwingStorage.write(
        key: 'profile-alpha-nickname',
        value: 'Alpha',
      );
      await options.setStringList(
        'profilesV2',
        const <String>['profile-beta'],
      );
      await options.setString('activeProfile', 'profile-beta');
      await options.setStringList(
        'profileDeletionTombstones',
        const <String>['profile-alpha'],
      );
      // Seed replacement storage too so init can load it.
      await throwingStorage.write(
        key: 'profile-beta-keypair',
        value: base64Encode(List<int>.filled(32, 2)),
      );
      await throwingStorage.write(
        key: 'profile-beta-peerId',
        value: '12D3KooWBetaPeerId11111111111111111111111111111111',
      );
      await throwingStorage.write(key: 'profile-beta-nickname', value: 'Beta');

      final controller =
          ProfilesController(storage: throwingStorage, options: options);
      await controller.init(const <String>[]);

      // The tombstone must be cleared and the storage records deleted.
      final tombstones =
          await options.getStringList('profileDeletionTombstones');
      expect(tombstones, isNot(contains('profile-alpha')),
          reason: 'startup must redrive the cleanup and clear the '
              'tombstone once the storage delete succeeds');
      expect(await throwingStorage.read(key: 'profile-alpha-keypair'), isNull,
          reason: 'startup retry must actually delete the private-key record');

      // The replacement must be selected as active; the deleted id must
      // not be in the in-memory map.
      expect(controller.activeProfile, 'profile-beta',
          reason: 'the replacement that was committed before the crash '
              'must remain the active profile after the recovery startup');
      expect(controller.profiles, isNot(contains('profile-alpha')));
      expect(controller.profiles, contains('profile-beta'));
    });

    // ---- Comment 1 (follow-up): replacement-creation failure phase ----
    test(
        'replacement-creation failure is wrapped as `replacementCreate`, '
        'never `storageCleanup`; the active profile is untouched', () async {
      // Regression for the cleanup-misrouting bug: a generic catch-all
      // in the UI previously wrapped ANY non-ProfileDeletionException
      // failure as `storageCleanup`, exposing the destructive "Retry
      // Cleanup" button. When the underlying failure was a
      // replacement-creation error (key generation or persistence),
      // the still-live active profile was at risk: a user tapping
      // "Retry Cleanup" would have called
      // retryDeletionCleanup(activeProfileId), which (before its
      // hardening) deleted the active private key. The fix wraps the
      // failure in a distinct `replacementCreate` phase so the UI
      // cannot offer the destructive button on it. This test pins
      // the wrapper so a future regression cannot re-introduce the
      // misroute.
      const storage = FlutterSecureStorage();
      final options = buildOptions();
      // Seed a SINGLE profile so removeProfile is forced into the
      // replacement-creation branch.
      final controller = await seedController(
        storage: storage,
        options: options,
        profiles: <SeedProfile>[
          realisticSeed('profile-alpha'),
        ],
        activeProfileId: 'profile-alpha',
      );
      // Inject a deterministic failure at `_persistProfileIds` (used
      // by `_createProfile`). In unit-test scope `generateKeys()`
      // also fails because the Rust runtime is not loaded; both
      // paths exercise the catch and both must be wrapped.
      options.throwOnSetStringListKey = 'profilesV2';
      final telepathy = _RecordingTelepathy();

      await expectLater(
        controller.removeProfile('profile-alpha', telepathy: telepathy),
        throwsA(
          isA<ProfileDeletionException>()
              .having((e) => e.phase, 'phase',
                  ProfileDeletionPhase.replacementCreate)
              .having((e) => e.tombstonedForStartupRetry,
                  'tombstonedForStartupRetry', isFalse),
        ),
      );

      // The active profile's keypair MUST remain intact: replacement
      // creation happens BEFORE any destructive change to the
      // original profile.
      final keypair = await storage.read(key: 'profile-alpha-keypair');
      expect(keypair, isNotNull,
          reason: 'a replacement-creation failure must never delete '
              'the active profile private key');

      // No tombstone for the active profile: the operation aborted
      // before any durable intent was written for it.
      final tombstones =
          await options.getStringList('profileDeletionTombstones') ??
              const <String>[];
      expect(tombstones, isNot(contains('profile-alpha')),
          reason: 'a replacement-creation failure must not tombstone '
              'the still-live active profile');

      // The active profile is still in the in-memory map and still
      // active: no destructive change happened.
      expect(controller.profiles, contains('profile-alpha'));
      expect(controller.activeProfile, 'profile-alpha');

      // The backend transaction never started: replacement creation
      // happens BEFORE the gate is acquired.
      expect(telepathy.beginCalls, isEmpty,
          reason: 'begin must not run when replacement creation fails; '
              'no slot should be reserved');
      expect(telepathy.commitCalls, isEmpty);
      expect(telepathy.cancelCalls, isEmpty);
    });

    // ---- Comment 1 (follow-up): hardened cleanup retry rejection ----
    test(
        'retryDeletionCleanup rejects a cleanup request against a live '
        'active profile and leaves its keypair byte-for-byte intact', () async {
      // Safety net for the misrouting bug: even if a tombstone exists
      // for an id that is still live (e.g. left by a buggy older code
      // path or a mis-classified error from a future regression), the
      // hardened retry MUST refuse to delete secure records. The
      // tombstone is journal intent, NOT authorization to destroy a
      // profile that the in-memory map AND the persisted index still
      // claim exists.
      const storage = FlutterSecureStorage();
      final options = SharedPreferencesAsync();
      final controller = await seedController(
        storage: storage,
        options: options,
        profiles: <SeedProfile>[
          realisticSeed('profile-alpha'),
          realisticSeed('profile-beta'),
        ],
        activeProfileId: 'profile-alpha',
      );

      // Simulate a stale/mis-routed tombstone: the journal says
      // cleanup is pending even though the id is still live in both
      // the in-memory map AND the persisted index. This is exactly
      // the state a mis-classified replacement-creation failure would
      // have produced under the old UI.
      await options.setStringList(
        'profileDeletionTombstones',
        const <String>['profile-alpha'],
      );

      // Snapshot the keypair so the assertion can prove no mutation.
      final keypairBefore = await storage.read(key: 'profile-alpha-keypair');
      expect(keypairBefore, isNotNull,
          reason: 'test setup: the active profile keypair must be '
              'seeded before the retry is attempted');

      final result = await controller.retryDeletionCleanup('profile-alpha');

      expect(result, isFalse,
          reason: 'a retry against an id that is still in the index '
              'must be rejected without mutation');

      // The active profile's keypair MUST remain byte-for-byte intact.
      final keypairAfter = await storage.read(key: 'profile-alpha-keypair');
      expect(keypairAfter, keypairBefore,
          reason: 'the rejected retry must not mutate the active '
              'profile private key');

      // The tombstone journal must be unchanged: the retry was a
      // no-op. (Startup's `_retryTombstonedDeletions` will reconcile
      // the stale tombstone separately because the id is still in
      // the index; the runtime retry refuses to do destructive work
      // that the journal does not authorize.)
      final tombstones =
          await options.getStringList('profileDeletionTombstones') ??
              const <String>[];
      expect(tombstones, contains('profile-alpha'),
          reason: 'a rejected retry must not mutate the tombstone '
              'journal');

      // The in-memory map and persisted index are unchanged.
      expect(controller.profiles, contains('profile-alpha'));
      final persistedIndex =
          await options.getStringList('profilesV2') ?? const <String>[];
      expect(persistedIndex, contains('profile-alpha'));
    });

    test(
        'retryDeletionCleanup rejects a cleanup request when no durable '
        'tombstone exists, even if the id is absent from the index', () async {
      // Without a tombstone the cleanup is not authorized — the
      // request may be mis-routed (e.g. the user manually constructed
      // a request, or a future regression calls retry on an id that
      // was never deleted). The retry must refuse rather than
      // speculatively destroy storage.
      const storage = FlutterSecureStorage();
      final options = SharedPreferencesAsync();
      // Seed one profile, then manually remove it from the index AND
      // storage so the only invariant missing is the tombstone.
      // This is an unusual state but exactly what a mis-routed retry
      // would target.
      final controller = await seedController(
        storage: storage,
        options: options,
        profiles: <SeedProfile>[
          realisticSeed('profile-alpha'),
          realisticSeed('profile-beta'),
        ],
        activeProfileId: 'profile-alpha',
      );

      // Remove 'profile-beta' from in-memory + persisted index + storage
      // WITHOUT writing a tombstone (simulating an id the journal does
      // not know about).
      controller.profiles.remove('profile-beta');
      await options.setStringList(
        'profilesV2',
        const <String>['profile-alpha'],
      );
      await storage.delete(key: 'profile-beta-keypair');

      // Sanity: the tombstone journal is empty.
      final tombstonesBefore =
          await options.getStringList('profileDeletionTombstones') ??
              const <String>[];
      expect(tombstonesBefore, isNot(contains('profile-beta')));

      final result = await controller.retryDeletionCleanup('profile-beta');

      expect(result, isFalse,
          reason: 'a retry without a durable tombstone must be '
              'rejected; the journal is the authorization for '
              'cleanup, not the absence of the id in the index');

      // Tombstone journal must remain empty (no speculative writes).
      final tombstonesAfter =
          await options.getStringList('profileDeletionTombstones') ??
              const <String>[];
      expect(tombstonesAfter, isNot(contains('profile-beta')));
    });
  });
}

/// [FlutterSecureStorage] subclass that can be staged to throw on `delete`
/// so fault-injection tests can drive the REAL [ProfilesController]
/// through its storage-cleanup failure paths without test-only hooks in
/// the production controller.
class _ThrowingFlutterSecureStorage extends FlutterSecureStorage {
  bool throwOnDelete = false;

  @override
  Future<void> delete({
    required String key,
    AppleOptions? iOptions,
    AndroidOptions? aOptions,
    LinuxOptions? lOptions,
    WebOptions? webOptions,
    AppleOptions? mOptions,
    WindowsOptions? wOptions,
  }) async {
    if (throwOnDelete) {
      throw Exception('injected delete failure on $key');
    }
    return super.delete(key: key);
  }
}

/// [SharedPreferencesAsync] subclass that can be staged to throw on
/// `setString` or `setStringList`. Used by the two-phase transaction tests
/// to drive the REAL [ProfilesController] through its rollback paths
/// without test-only hooks in the production controller.
///
/// Key-based flags (`throwOnSetStringKey` / `throwOnSetStringListKey`)
/// inject failures ONLY on a specific shared-preferences key so a test
/// can fault the profile-index write while letting the tombstone write
/// (a different key) succeed. Whole-method flags remain for back-compat.
// ignore: must_be_immutable
class _ThrowingSharedPreferences extends SharedPreferencesAsync {
  bool throwOnSetString = false;
  bool throwOnSetStringList = false;
  String? throwOnSetStringKey;
  String? throwOnSetStringListKey;
  String? throwOnceOnSetStringKey;
  String? throwAfterSetStringListKey;
  final List<({String key, Object value})> successfulWrites =
      <({String key, Object value})>[];

  @override
  Future<void> setString(String key, String value) async {
    if (key == throwOnceOnSetStringKey) {
      throwOnceOnSetStringKey = null;
      throw Exception('injected one-shot setString failure on $key');
    }
    if (throwOnSetString || key == throwOnSetStringKey) {
      throw Exception('injected setString failure on $key');
    }
    await super.setString(key, value);
    successfulWrites.add((key: key, value: value));
  }

  @override
  Future<void> setStringList(String key, List<String> value) async {
    if (throwOnSetStringList || key == throwOnSetStringListKey) {
      throw Exception('injected setStringList failure on $key');
    }
    await super.setStringList(key, value);
    successfulWrites.add((key: key, value: List<String>.from(value)));
    if (key == throwAfterSetStringListKey) {
      throw Exception('injected post-write setStringList failure on $key');
    }
  }
}

class _RecordingRustApi implements RustLibApi {
  int generateKeysCalls = 0;

  @override
  (String, Uint8List) crateFlutterUtilsGenerateKeys() {
    generateKeysCalls += 1;
    return (
      '12D3KooWGeneratedPeerId000000000000000000000000000000',
      Uint8List.fromList(List<int>.filled(32, 7)),
    );
  }

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

class _BeginRecord {
  _BeginRecord({required this.key, required this.contacts});

  final List<int> key;
  final List<Contact> contacts;
}

class _MutableContact implements Contact {
  _MutableContact({
    required String id,
    required String nickname,
    required String peerId,
    double outputVolume = 0,
    this.onClone,
  })  : _id = id,
        _nickname = nickname,
        _peerId = peerId,
        _outputVolume = outputVolume;

  final String _id;
  final String _peerId;
  final void Function()? onClone;
  String _nickname;
  double _outputVolume;

  @override
  String id() => _id;

  @override
  Future<PublicKey> getPeerId() => throw UnimplementedError();

  @override
  bool idEq({required List<int> id}) => false;

  @override
  String nickname() => _nickname;

  @override
  double outputVolume() => _outputVolume;

  @override
  String peerId() => _peerId;

  @override
  Contact pubClone() {
    onClone?.call();
    return _MutableContact(
      id: _id,
      nickname: _nickname,
      peerId: _peerId,
      outputVolume: _outputVolume,
    );
  }

  @override
  void setNickname({required String nickname}) {
    _nickname = nickname;
  }

  @override
  void setOutputVolume({required double decibel}) {
    _outputVolume = decibel;
  }

  @override
  bool get isDisposed => false;

  @override
  void dispose() {}
}

class _RecordingTelepathy implements Telepathy {
  final List<_BeginRecord> beginCalls = <_BeginRecord>[];
  final List<void> commitCalls = <void>[];
  final List<void> cancelCalls = <void>[];
  final Completer<void> beginIdentitySwitchEntered = Completer<void>();

  void Function()? onBegin;
  Future<void> Function()? onCommit;
  Object? commitException;
  Completer<void>? _beginIdentitySwitchRelease;

  void pauseBeginIdentitySwitch() {
    _beginIdentitySwitchRelease = Completer<void>();
  }

  void releaseBeginIdentitySwitch() {
    _beginIdentitySwitchRelease!.complete();
  }

  @override
  Future<void> beginIdentitySwitch({
    required List<int> targetKey,
    required List<Contact> targetContacts,
  }) async {
    beginCalls.add(_BeginRecord(key: targetKey, contacts: targetContacts));
    onBegin?.call();
    if (!beginIdentitySwitchEntered.isCompleted) {
      beginIdentitySwitchEntered.complete();
    }
    await _beginIdentitySwitchRelease?.future;
  }

  @override
  Future<void> commitIdentitySwitch() async {
    commitCalls.add(null);
    await onCommit?.call();
    final exception = commitException;
    if (exception != null) {
      throw exception;
    }
  }

  @override
  Future<void> recoverIdentitySwitch() async {}

  @override
  Future<void> cancelIdentitySwitch() async {
    cancelCalls.add(null);
  }

  @override
  bool get isDisposed => false;

  @override
  void dispose() {}

  @override
  Future<void> audioTest() async {}

  @override
  ChatMessage buildChat({
    required Contact contact,
    required String text,
    required List<(String, Uint8List)> attachments,
  }) =>
      throw UnimplementedError();

  @override
  Future<void> endCall() async {}

  @override
  Future<void> joinRoom(
      {required List<String> memberStrings,
      required StartOperation operation}) async {}

  @override
  Future<(List<AudioDevice>, List<AudioDevice>)> listDevices() async =>
      (<AudioDevice>[], <AudioDevice>[]);

  @override
  StartOperation newStartOperation() => throw UnimplementedError();

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
  Future<void> startCall(
      {required Contact contact, required StartOperation operation}) async {}

  @override
  Future<void> startManager() async {}

  @override
  Future<void> startScreenshare({required Contact contact}) async {}

  @override
  Future<void> startSession({required Contact contact}) async {}

  @override
  Future<void> stopSession({required Contact contact}) async {}
}
