import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:telepathy/core/rust/flutter.dart';
import 'package:telepathy/core/rust/flutter/utils.dart';
import 'package:telepathy/core/rust/types.dart';
import 'package:telepathy/core/utils/console.dart';
import 'package:telepathy/models/index.dart';
import 'package:uuid/uuid.dart';

typedef RoomHasher = String Function({required List<String> peers});

class ProfilesController with ChangeNotifier {
  static const String _profilesKey = 'profilesV2';
  static const String _activeProfileKey = 'activeProfile';
  static const String _defaultProfileNickname = 'Default';
  static const String _unnamedProfileNickname = 'Unnamed Profile';
  static const double _minContactOutputVolumeDb = -15.0;
  static const double _maxContactOutputVolumeDb = 15.0;
  static const String _deletionTombstonesKey = 'profileDeletionTombstones';

  /// Iroh SecretKey length. Profiles whose persisted keypair does not decode
  /// to exactly this many bytes are rejected at load time so they cannot
  /// become switch targets — a malformed target previously caused the
  /// backend's commit_identity_switch to fail AFTER the slot was reserved,
  /// wedging the call slot.
  static const int identityKeyLength = 32;

  final FlutterSecureStorage storage;
  final SharedPreferencesAsync options;
  final RoomHasher roomHasher;

  ProfilesController({
    required this.storage,
    required this.options,
    this.roomHasher = roomHash,
  });

  /// The ids of all available profiles.
  Map<String, Profile> profiles = <String, Profile>{};

  /// The id of the active profile. Empty until [init] completes.
  ///
  /// Read-only outside this controller: runtime profile changes must go
  /// through [switchActiveProfile] (or active-profile deletion) so the
  /// Rust backend's identity invariant stays in lockstep with the
  /// frontend's persisted active profile. The previous public setter
  /// bypassed the backend entirely, leaving the frontend pointing at a
  /// profile whose signing key the backend had never installed.
  String _activeProfile = '';

  String get activeProfile => _activeProfile;

  /// True while a prepared identity switch is in flight.
  bool _isIdentitySwitchPending = false;

  bool get isIdentitySwitchPending => _isIdentitySwitchPending;

  bool _initialized = false;
  bool _disposed = false;
  Future<void> _operationQueue = Future<void>.value();

  bool get isInitialized => _initialized;

  bool get hasActiveProfile => profiles.containsKey(activeProfile);

  Profile get currentProfile => _currentProfile();

  Map<String, Contact> get contacts => _currentProfile().contacts;

  Map<String, Room> get rooms => _currentProfile().rooms;

  List<int> get keypair => _currentProfile().keypair;

  String get peerId => _currentProfile().peerId;

  @override
  void dispose() {
    _disposed = true;
    super.dispose();
  }

  Future<void> init(List<String> args) {
    return _enqueue(() => _init(args));
  }

  Future<void> _init(List<String> args) async {
    _initialized = false;
    _activeProfile = '';
    profiles = <String, Profile>{};

    // Honor deletion tombstones before loading profiles so orphaned records
    // cannot become switch targets. The profile index is authoritative: an
    // indexed id is live, while an unindexed tombstoned id needs cleanup.
    await _retryTombstonedDeletions();

    final List<String> profileIds = _dedupe(
      await _getStringListOption(_profilesKey) ?? const <String>[],
    );

    final List<String> badProfileIds = <String>[];

    for (final String id in profileIds) {
      try {
        final Profile? profile = await _loadProfile(id);
        if (profile == null) {
          badProfileIds.add(id);
          continue;
        }
        profiles[id] = profile;
      } catch (error, stackTrace) {
        DebugConsole.warn(
          'profile $id failed to load due to storage error: $error\n$stackTrace',
        );
        rethrow;
      }
    }

    if (badProfileIds.isNotEmpty) {
      DebugConsole.warn(
        'Ignoring invalid profiles from $_profilesKey: ${badProfileIds.join(', ')}',
      );
      // Keep secure-storage data intact. Only repair the profile index so startup
      // does not repeatedly trip over corrupted or half-written records.
      await _persistProfileIds();
    }

    if (profiles.isEmpty) {
      final String defaultId = await _createProfile(
        _defaultProfileNickname,
        notify: false,
      );
      await _setActiveProfile(defaultId, notify: false);
    } else {
      String selectedId = await _getStringOption(_activeProfileKey) ?? '';
      if (!profiles.containsKey(selectedId)) {
        selectedId = profiles.keys.first;
      }

      final String? override = args.elementAtOrNull(0);
      if (override != null && override.trim().isNotEmpty) {
        MapEntry<String, Profile>? match;
        for (final MapEntry<String, Profile> entry in profiles.entries) {
          if (entry.key == override || entry.value.nickname == override) {
            match = entry;
            break;
          }
        }
        if (match != null) {
          selectedId = match.key;
        } else {
          DebugConsole.warn('Profile override not found: $override');
        }
      }

      await _setActiveProfile(selectedId, notify: false);
    }

    _initialized = true;
    _safeNotifyListeners();
  }

  /// This can still throw when the peer id is invalid because the historical API
  /// returns a non-null [Contact]. Use [tryAddContact] when taking user input.
  Contact addContact(
    String nickname,
    String peerId, {
    String? directInvitation,
  }) {
    final Profile profile = _currentProfile();

    late final Contact contact;
    late final String contactId;
    try {
      contact = Contact(nickname: nickname, peerId: peerId);
      if (directInvitation != null) {
        contact.setDirectInvitation(invitation: directInvitation);
        contact.setDirect(isDirect: true);
      }
      contactId = contact.id();
    } catch (error, stackTrace) {
      DebugConsole.warn('invalid contact: $error\n$stackTrace');
      rethrow;
    }

    profile.contacts[contactId] = contact;
    _safeNotifyListeners();
    unawaited(_enqueue(() => _saveContactsFor(profile.id, notify: false)));
    return contact;
  }

  Contact? tryAddContact(
    String nickname,
    String peerId, {
    String? directInvitation,
  }) {
    try {
      return addContact(
        nickname,
        peerId,
        directInvitation: directInvitation,
      );
    } catch (error) {
      DebugConsole.warn('contact was not added: $error');
      return null;
    }
  }

  Contact? getContact(String id) {
    return contacts[id];
  }

  void removeContact(Contact contact) {
    final Profile profile = _currentProfile();

    late final String contactId;
    try {
      contactId = contact.id();
    } catch (error) {
      DebugConsole.warn(
          'contact was not removed because its id is invalid: $error');
      return;
    }

    if (profile.contacts.remove(contactId) != null) {
      _safeNotifyListeners();
      unawaited(_enqueue(() => _saveContactsFor(profile.id, notify: false)));
    }
  }

  /// Saves the contacts for the active profile at call time.
  Future<void> saveContacts() {
    final String profileId = _currentProfile().id;
    _safeNotifyListeners();
    return _enqueue(() => _saveContactsFor(profileId, notify: false));
  }

  Room addRoom(String nickname, List<String> peerIds) {
    final Profile profile = _currentProfile();
    final List<String> roomPeerIds = List<String>.from(peerIds);

    late final Room room;
    try {
      room = Room(
        id: roomHasher(peers: roomPeerIds),
        peerIds: roomPeerIds,
        nickname: nickname,
      );
    } catch (error, stackTrace) {
      DebugConsole.warn('invalid room: $error\n$stackTrace');
      rethrow;
    }

    profile.rooms[room.id] = room;
    _safeNotifyListeners();
    unawaited(_enqueue(() => _saveRoomsFor(profile.id, notify: false)));
    return room;
  }

  Room? tryAddRoom(String nickname, List<String> peerIds) {
    try {
      return addRoom(nickname, peerIds);
    } catch (error) {
      DebugConsole.warn('room was not added: $error');
      return null;
    }
  }

  void removeRoom(Room room) {
    final Profile profile = _currentProfile();

    Room? removedRoom = profile.rooms.remove(room.id);

    if (removedRoom == null) {
      String? roomKey;
      for (final MapEntry<String, Room> entry in profile.rooms.entries) {
        if (identical(entry.value, room)) {
          roomKey = entry.key;
          break;
        }
      }

      if (roomKey == null) {
        return;
      }

      removedRoom = profile.rooms.remove(roomKey);
    }

    if (removedRoom == null) {
      return;
    }

    _safeNotifyListeners();
    unawaited(_enqueue(() => _saveRoomsFor(profile.id, notify: false)));
  }

  Future<void> saveRooms() {
    final String profileId = _currentProfile().id;
    _safeNotifyListeners();
    return _enqueue(() => _saveRoomsFor(profileId, notify: false));
  }

  Future<String> createProfile(String nickname) {
    return _enqueue(() => _createProfile(nickname));
  }

  Future<String> _createProfile(String nickname, {bool notify = true}) async {
    final String cleanNickname =
        nickname.trim().isEmpty ? _unnamedProfileNickname : nickname;
    final String id = const Uuid().v4();

    try {
      late final String peerId;
      late final Uint8List keypair;
      try {
        (peerId, keypair) = generateKeys();
      } catch (error, stackTrace) {
        DebugConsole.warn(
            'failed to generate profile keys: $error\n$stackTrace');
        rethrow;
      }

      final Profile profile = Profile(
        id: id,
        nickname: cleanNickname,
        peerId: peerId,
        keypair: keypair,
        contacts: <String, Contact>{},
        rooms: <String, Room>{},
      );

      profiles[id] = profile;
      await _writeProfile(profile);
      await _persistProfileIds();
    } catch (error, stackTrace) {
      await _cleanupFailedProfileCreation(id);
      Error.throwWithStackTrace(error, stackTrace);
    }

    if (notify) {
      _safeNotifyListeners();
    }

    return id;
  }

  Future<void> removeProfile(String id, {required Telepathy telepathy}) {
    return _enqueue(() => _removeProfile(id, telepathy: telepathy));
  }

  Future<void> _removeProfile(String id, {required Telepathy telepathy}) async {
    if (!profiles.containsKey(id)) {
      DebugConsole.warn('removeProfile called for unknown profile: $id');
      return;
    }

    if (id == _activeProfile) {
      final String replacementId = await _replacementProfileId(id);
      await _switchActiveProfile(replacementId, telepathy: telepathy);
    }

    await _removeInactiveProfile(id);
  }

  Future<String> _replacementProfileId(String deletedId) async {
    for (final String id in profiles.keys) {
      if (id != deletedId) {
        return id;
      }
    }
    return _createProfile(_defaultProfileNickname, notify: false);
  }

  Future<void> _removeInactiveProfile(String id) async {
    await _recordDeletionIntent(id);
    final List<String> remainingIds = profiles.keys
        .where((String profileId) => profileId != id)
        .toList(growable: false);
    await _persistProfileIds(remainingIds);
    profiles.remove(id);

    try {
      await _deleteProfileStorage(id);
      await _clearDeletionIntent(id);
    } finally {
      _safeNotifyListeners();
    }
  }

  /// Switches active profile through a prepared identity token.
  /// Persists target active id, updates memory, then commits target identity.
  ///
  /// This is the ONLY public runtime path that mutates the active profile:
  /// the previous `setActiveProfile` setter bypassed the backend entirely,
  /// so the frontend could end up pointing at a profile whose signing key
  /// the backend had never installed. Startup selection uses the private
  /// [_setActiveProfile] helper instead.
  Future<void> switchActiveProfile(
    String id, {
    required Telepathy telepathy,
  }) {
    return _enqueue(() => _switchActiveProfile(id, telepathy: telepathy));
  }

  Future<void> _switchActiveProfile(
    String id, {
    required Telepathy telepathy,
  }) async {
    if (!profiles.containsKey(id)) {
      DebugConsole.warn('switch to unknown profile: $id');
      return;
    }
    if (id == _activeProfile) {
      return;
    }

    final Profile target = profiles[id]!;

    _isIdentitySwitchPending = true;
    _safeNotifyListeners();

    try {
      final List<Contact> snapshot =
          target.contacts.values.map((Contact c) => c.pubClone()).toList();
      final PreparedIdentitySwitch prepared =
          await telepathy.prepareIdentitySwitch(
        targetKey: target.keypair,
        targetContacts: snapshot,
      );
      try {
        await _setStringOption(_activeProfileKey, id);
      } catch (error, stackTrace) {
        prepared.dispose();
        Error.throwWithStackTrace(error, stackTrace);
      }

      _activeProfile = id;
      await prepared.commit();
    } finally {
      _isIdentitySwitchPending = false;
      _safeNotifyListeners();
    }
  }

  Future<void> _setActiveProfile(String id, {bool notify = true}) async {
    String targetId = id;

    if (!profiles.containsKey(targetId)) {
      if (profiles.isEmpty) {
        targetId = await _createProfile(_defaultProfileNickname, notify: false);
      } else {
        DebugConsole.warn(
            'active profile id not found: $id; using first profile');
        targetId = profiles.keys.first;
      }
    }

    final String previousActive = activeProfile;
    _activeProfile = targetId;

    try {
      await _setStringOption(_activeProfileKey, targetId);
    } catch (error, stackTrace) {
      _activeProfile = previousActive;
      Error.throwWithStackTrace(error, stackTrace);
    }

    if (notify) {
      _safeNotifyListeners();
    }
  }

  Future<Map<String, Contact>> loadContacts(String id) async {
    final Map<String, Contact> contacts = <String, Contact>{};
    bool needsDirectInvitationMigration = false;
    final String? contactsStr = await _readStorage('$id-contacts');

    if (contactsStr == null || contactsStr.trim().isEmpty) {
      return contacts;
    }

    final Map<String, dynamic> contactsMap = _decodeJsonMap(
      contactsStr,
      '$id-contacts',
    );

    for (final MapEntry<String, dynamic> entry in contactsMap.entries) {
      final Map<String, dynamic>? contactMap = _asMap(entry.value);
      if (contactMap == null) {
        DebugConsole.warn('invalid contact record for ${entry.key}: not a map');
        continue;
      }

      final Object? nickname = contactMap['nickname'];
      final Object? peerId = contactMap['peerId'];
      final double outputVolume = _normalizeContactOutputVolume(
        contactMap['outputVolume'],
        entry.key,
      );

      if (nickname is! String || peerId is! String) {
        DebugConsole.warn(
            'invalid contact record for ${entry.key}: missing nickname or peerId');
        continue;
      }

      final bool isDirect = contactMap['isDirect'] == true;
      final bool hasLegacyInvitationKey =
          contactMap.containsKey('directConnectionString');
      final Object? persistedInvitation =
          contactMap.containsKey('directInvitation')
              ? contactMap['directInvitation']
              : contactMap['directConnectionString'];
      final String? directInvitation =
          persistedInvitation is String ? persistedInvitation : null;

      try {
        final Contact contact = Contact.fromParts(
          id: entry.key,
          nickname: nickname,
          peerId: peerId,
          outputVolume: outputVolume,
          isDirect: isDirect,
          directInvitation: directInvitation,
        );
        contacts[entry.key] = contact;
        needsDirectInvitationMigration |= hasLegacyInvitationKey ||
            (persistedInvitation != null && persistedInvitation is! String) ||
            directInvitation != contact.directInvitation() ||
            isDirect != contact.isDirect();
      } catch (error) {
        DebugConsole.warn('invalid contact format for ${entry.key}: $error');
      }
    }

    if (needsDirectInvitationMigration) {
      try {
        await _writeStorage(
          key: '$id-contacts',
          value: jsonEncode(_serializeContacts(contacts)),
        );
      } catch (error) {
        DebugConsole.warn(
          'failed to persist direct invitation migration for profile $id: '
          '$error',
        );
      }
    }

    return contacts;
  }

  Future<Map<String, Room>> loadRooms(String id) async {
    final Map<String, Room> rooms = <String, Room>{};
    final String? roomStr = await _readStorage('$id-rooms');

    if (roomStr == null || roomStr.trim().isEmpty) {
      return rooms;
    }

    final Map<String, dynamic> roomMap = _decodeJsonMap(roomStr, '$id-rooms');

    for (final MapEntry<String, dynamic> entry in roomMap.entries) {
      final Map<String, dynamic>? value = _asMap(entry.value);
      if (value == null) {
        DebugConsole.warn('invalid room record for ${entry.key}: not a map');
        continue;
      }

      try {
        rooms[entry.key] = Room.fromJson(value);
      } catch (error) {
        DebugConsole.warn('invalid room format for ${entry.key}: $error');
      }
    }

    return rooms;
  }

  Future<Profile?> _loadProfile(String id) async {
    if (id.trim().isEmpty) {
      DebugConsole.warn('ignoring empty profile id');
      return null;
    }

    final String? keyStr = await _readStorage('$id-keypair');
    final String? peerId = await _readStorage('$id-peerId');

    if (keyStr == null || keyStr.trim().isEmpty) {
      DebugConsole.warn('profile $id is missing keypair');
      return null;
    }

    if (peerId == null || peerId.trim().isEmpty) {
      DebugConsole.warn('profile $id is missing peerId');
      return null;
    }

    late final List<int> keyBytes;
    try {
      keyBytes = base64Decode(keyStr);
    } catch (error) {
      DebugConsole.warn('profile $id has invalid base64 keypair: $error');
      return null;
    }

    if (keyBytes.length != identityKeyLength) {
      DebugConsole.warn(
        'profile $id has malformed keypair: expected '
        '$identityKeyLength bytes, got ${keyBytes.length}; '
        'rejecting so this profile cannot become a switch target',
      );
      return null;
    }

    final String nickname =
        await _readStorage('$id-nickname') ?? _unnamedProfileNickname;

    return Profile(
      id: id,
      nickname: nickname.trim().isEmpty ? _unnamedProfileNickname : nickname,
      peerId: peerId,
      keypair: keyBytes,
      contacts: await loadContacts(id),
      rooms: await loadRooms(id),
    );
  }

  Future<void> _saveContactsFor(String profileId, {bool notify = true}) async {
    final Profile? profile = profiles[profileId];
    if (profile == null) {
      DebugConsole.warn('cannot save contacts for missing profile: $profileId');
      return;
    }

    if (notify) {
      _safeNotifyListeners();
    }

    await _writeStorage(
      key: '$profileId-contacts',
      value: jsonEncode(_serializeContacts(profile.contacts)),
    );
  }

  Map<String, Map<String, dynamic>> _serializeContacts(
    Map<String, Contact> contacts,
  ) {
    final Map<String, Map<String, dynamic>> contactsMap =
        <String, Map<String, dynamic>>{};

    for (final MapEntry<String, Contact> entry in contacts.entries) {
      try {
        final String? directInvitation = entry.value.directInvitation();
        final bool hasCanonicalInvitation =
            directInvitation?.startsWith('tp1:') == true;
        contactsMap[entry.key] = <String, dynamic>{
          'nickname': entry.value.nickname(),
          'peerId': entry.value.peerId(),
          'outputVolume': entry.value.outputVolume(),
          'isDirect': hasCanonicalInvitation && entry.value.isDirect(),
          if (hasCanonicalInvitation) 'directInvitation': directInvitation,
        };
      } catch (error) {
        DebugConsole.warn('skipping contact ${entry.key} during save: $error');
      }
    }

    return contactsMap;
  }

  Future<void> _saveRoomsFor(String profileId, {bool notify = true}) async {
    final Profile? profile = profiles[profileId];
    if (profile == null) {
      DebugConsole.warn('cannot save rooms for missing profile: $profileId');
      return;
    }

    if (notify) {
      _safeNotifyListeners();
    }

    final Map<String, Map<String, dynamic>> roomMap =
        <String, Map<String, dynamic>>{};

    for (final MapEntry<String, Room> entry in profile.rooms.entries) {
      try {
        roomMap[entry.key] = entry.value.toJson();
      } catch (error) {
        DebugConsole.warn('skipping room ${entry.key} during save: $error');
      }
    }

    await _writeStorage(
      key: '$profileId-rooms',
      value: jsonEncode(roomMap),
    );
  }

  Future<void> _writeProfile(Profile profile) async {
    await _writeStorage(
      key: '${profile.id}-keypair',
      value: base64Encode(profile.keypair),
    );
    await _writeStorage(key: '${profile.id}-peerId', value: profile.peerId);
    await _writeStorage(key: '${profile.id}-contacts', value: jsonEncode({}));
    await _writeStorage(key: '${profile.id}-rooms', value: jsonEncode({}));
    await _writeStorage(key: '${profile.id}-nickname', value: profile.nickname);
  }

  Future<void> _cleanupFailedProfileCreation(String id) async {
    try {
      await _recordDeletionIntent(id);
    } catch (error, stackTrace) {
      DebugConsole.warn(
        'failed to tombstone profile $id after create error; retaining its '
        'index and storage: $error\n$stackTrace',
      );
      return;
    }

    profiles.remove(id);
    try {
      await _persistProfileIds();
    } catch (error, stackTrace) {
      DebugConsole.warn(
        'failed to exclude profile $id from the profile index after create '
        'error; retaining its storage: $error\n$stackTrace',
      );
      return;
    }

    try {
      await _deleteProfileStorage(id);
      await _clearDeletionIntent(id);
    } catch (error, stackTrace) {
      DebugConsole.warn(
        'failed to delete profile $id storage after create error: '
        '$error\n$stackTrace',
      );
    }
  }

  Future<void> _deleteProfileStorage(String id) async {
    final List<String> keys = <String>[
      '$id-keypair',
      '$id-peerId',
      '$id-contacts',
      '$id-rooms',
      '$id-nickname',
    ];
    Object? firstError;
    StackTrace? firstStackTrace;

    for (final String key in keys) {
      try {
        await _deleteStorage(key);
      } catch (error, stackTrace) {
        firstError ??= error;
        firstStackTrace ??= stackTrace;
      }
    }

    if (firstError != null) {
      Error.throwWithStackTrace(firstError, firstStackTrace!);
    }
  }

  Future<void> _persistProfileIds([List<String>? ids]) async {
    await _setStringListOption(
      _profilesKey,
      ids ?? profiles.keys.toList(growable: false),
    );
  }

  /// Records a durable deletion intent for `id`. MUST be called before
  /// removing `id` from the profile index or deleting its secure-storage
  /// records. Startup's [_retryTombstonedDeletions] honors this journal
  /// before loading profiles and redrives cleanup until it succeeds.
  Future<void> _recordDeletionIntent(String id) async {
    final List<String> tombstones = _dedupe(
      await _getStringListOption(_deletionTombstonesKey) ?? const <String>[],
    );
    if (!tombstones.contains(id)) {
      tombstones.add(id);
      await _setStringListOption(_deletionTombstonesKey, tombstones);
    }
  }

  /// Clears the deletion intent for `id`. Called only after the id's
  /// secure-storage records are confirmed removed.
  Future<void> _clearDeletionIntent(String id) async {
    final List<String> tombstones = _dedupe(
      await _getStringListOption(_deletionTombstonesKey) ?? const <String>[],
    );
    if (tombstones.isEmpty || !tombstones.contains(id)) {
      return;
    }
    tombstones.remove(id);
    await _setStringListOption(_deletionTombstonesKey, tombstones);
  }

  Future<void> _retryTombstonedDeletions() async {
    final List<String> indexedIds = _dedupe(
      await _getStringListOption(_profilesKey) ?? const <String>[],
    );
    final List<String> tombstones = _dedupe(
      await _getStringListOption(_deletionTombstonesKey) ?? const <String>[],
    );
    final List<String> remaining = <String>[];

    for (final String id in tombstones) {
      if (indexedIds.contains(id)) {
        continue;
      }
      try {
        await _deleteProfileStorage(id);
      } catch (error, stackTrace) {
        DebugConsole.warn(
          'startup storage cleanup retry failed for profile $id: '
          '$error\n$stackTrace',
        );
        remaining.add(id);
      }
    }
    await _setStringListOption(_deletionTombstonesKey, remaining);
  }

  Profile _currentProfile() {
    final Profile? profile = profiles[activeProfile];
    if (profile != null) {
      return profile;
    }

    if (profiles.isNotEmpty) {
      final String fallbackId = profiles.keys.first;
      DebugConsole.warn(
        'active profile "$activeProfile" is invalid; falling back to "$fallbackId"',
      );
      _activeProfile = fallbackId;
      unawaited(
        _enqueue(() => _setStringOption(_activeProfileKey, fallbackId)),
      );
      return profiles[fallbackId]!;
    }

    throw StateError(
      'ProfilesController has no profiles. Call init() before using profile data.',
    );
  }

  Future<T> _enqueue<T>(Future<T> Function() operation) {
    final Future<T> result = _operationQueue.then((_) => operation());

    _operationQueue = result.then<void>(
      (_) {},
      onError: (Object error, StackTrace stackTrace) {
        DebugConsole.warn('profile operation failed: $error\n$stackTrace');
      },
    );

    return result;
  }

  Future<String?> _readStorage(String key) async {
    try {
      return await storage.read(key: key);
    } catch (error, stackTrace) {
      DebugConsole.warn(
        'secure storage read failed for $key: $error\n$stackTrace',
      );
      rethrow;
    }
  }

  Future<void> _writeStorage(
      {required String key, required String value}) async {
    try {
      await storage.write(key: key, value: value);
    } catch (error, stackTrace) {
      DebugConsole.warn(
        'secure storage write failed for $key: $error\n$stackTrace',
      );
      rethrow;
    }
  }

  Future<void> _deleteStorage(String key) async {
    try {
      await storage.delete(key: key);
    } catch (error, stackTrace) {
      DebugConsole.warn(
        'secure storage delete failed for $key: $error\n$stackTrace',
      );
      rethrow;
    }
  }

  Future<String?> _getStringOption(String key) async {
    try {
      return await options.getString(key);
    } catch (error, stackTrace) {
      DebugConsole.warn('options read failed for $key: $error\n$stackTrace');
      rethrow;
    }
  }

  Future<List<String>?> _getStringListOption(String key) async {
    try {
      return await options.getStringList(key);
    } catch (error, stackTrace) {
      DebugConsole.warn(
        'options list read failed for $key: $error\n$stackTrace',
      );
      rethrow;
    }
  }

  Future<void> _setStringOption(String key, String value) async {
    try {
      await options.setString(key, value);
    } catch (error, stackTrace) {
      DebugConsole.warn('options write failed for $key: $error\n$stackTrace');
      rethrow;
    }
  }

  Future<void> _setStringListOption(String key, List<String> value) async {
    try {
      await options.setStringList(key, value);
    } catch (error, stackTrace) {
      DebugConsole.warn(
        'options list write failed for $key: $error\n$stackTrace',
      );
      rethrow;
    }
  }

  Map<String, dynamic> _decodeJsonMap(String encoded, String storageKey) {
    try {
      final Object? decoded = jsonDecode(encoded);
      final Map<String, dynamic>? map = _asMap(decoded);
      if (map == null) {
        DebugConsole.warn('invalid JSON for $storageKey: expected object');
        return <String, dynamic>{};
      }
      return map;
    } catch (error) {
      DebugConsole.warn('invalid JSON for $storageKey: $error');
      return <String, dynamic>{};
    }
  }

  Map<String, dynamic>? _asMap(Object? value) {
    if (value is Map<String, dynamic>) {
      return value;
    }
    if (value is Map) {
      return value.map<String, dynamic>(
        (dynamic key, dynamic value) => MapEntry<String, dynamic>(
          key.toString(),
          value,
        ),
      );
    }
    return null;
  }

  double? _asDouble(Object? value) {
    if (value is num) {
      return value.toDouble();
    }
    return null;
  }

  double _normalizeContactOutputVolume(Object? raw, String contactKey) {
    final double? parsed = _asDouble(raw);
    if (parsed == null) {
      return 0.0;
    }
    if (!parsed.isFinite ||
        parsed < _minContactOutputVolumeDb ||
        parsed > _maxContactOutputVolumeDb) {
      DebugConsole.warn(
        'invalid outputVolume for $contactKey: $raw; using 0.0',
      );
      return 0.0;
    }
    return parsed;
  }

  List<String> _dedupe(List<String> values) {
    final Set<String> seen = <String>{};
    final List<String> result = <String>[];

    for (final String value in values) {
      final String clean = value.trim();
      if (clean.isEmpty || !seen.add(clean)) {
        continue;
      }
      result.add(clean);
    }

    return result;
  }

  void _safeNotifyListeners() {
    if (!_disposed) {
      notifyListeners();
    }
  }
}
