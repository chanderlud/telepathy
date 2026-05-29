import 'dart:async';
import 'dart:convert';

import 'package:collection/collection.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:telepathy/core/rust/flutter/utils.dart';
import 'package:telepathy/core/rust/types.dart';
import 'package:telepathy/core/utils/console.dart';
import 'package:telepathy/models/index.dart';
import 'package:uuid/uuid.dart';

class ProfilesController with ChangeNotifier {
  static const String _profilesKey = 'profilesV2';
  static const String _activeProfileKey = 'activeProfile';
  static const String _defaultProfileNickname = 'Default';
  static const String _unnamedProfileNickname = 'Unnamed Profile';
  static const double _minContactOutputVolumeDb = -15.0;
  static const double _maxContactOutputVolumeDb = 15.0;

  final FlutterSecureStorage storage;
  final SharedPreferencesAsync options;

  ProfilesController({required this.storage, required this.options});

  /// The ids of all available profiles.
  Map<String, Profile> profiles = <String, Profile>{};

  /// The id of the active profile. Empty until [init] completes.
  String activeProfile = '';

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
    activeProfile = '';
    profiles = <String, Profile>{};

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
        final MapEntry<String, Profile>? match =
            profiles.entries.firstWhereOrNull(
          (entry) => entry.key == override || entry.value.nickname == override,
        );
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
  Contact addContact(String nickname, String peerId) {
    final Profile profile = _currentProfile();

    late final Contact contact;
    late final String contactId;
    try {
      contact = Contact(nickname: nickname, peerId: peerId);
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

  Contact? tryAddContact(String nickname, String peerId) {
    try {
      return addContact(nickname, peerId);
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

    late final Room room;
    try {
      room = Room(
        id: roomHash(peers: peerIds),
        peerIds: peerIds,
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

    late final String peerId;
    late final Uint8List keypair;
    try {
      (peerId, keypair) = generateKeys();
    } catch (error, stackTrace) {
      DebugConsole.warn('failed to generate profile keys: $error\n$stackTrace');
      rethrow;
    }

    final String id = const Uuid().v4();
    final Profile profile = Profile(
      id: id,
      nickname: cleanNickname,
      peerId: peerId,
      keypair: keypair,
      contacts: <String, Contact>{},
      rooms: <String, Room>{},
    );

    profiles[id] = profile;

    try {
      await _writeProfile(profile);
      await _persistProfileIds();
    } catch (error, stackTrace) {
      profiles.remove(id);
      try {
        await _deleteProfileStorage(id);
      } catch (cleanupError) {
        DebugConsole.warn(
          'failed to clean up profile $id after create error: $cleanupError',
        );
      }
      Error.throwWithStackTrace(error, stackTrace);
    }

    if (notify) {
      _safeNotifyListeners();
    }

    return id;
  }

  Future<void> removeProfile(String id) {
    return _enqueue(() => _removeProfile(id));
  }

  Future<void> _removeProfile(String id) async {
    if (!profiles.containsKey(id)) {
      DebugConsole.warn('removeProfile called for unknown profile: $id');
      return;
    }

    final Map<String, Profile> profilesBefore =
        Map<String, Profile>.from(profiles);
    final String activeBefore = activeProfile;
    final bool wasActive = activeProfile == id;

    profiles.remove(id);

    try {
      await _persistProfileIds();
      await _deleteProfileStorage(id);

      if (profiles.isEmpty) {
        final String defaultId = await _createProfile(
          _defaultProfileNickname,
          notify: false,
        );
        await _setActiveProfile(defaultId, notify: false);
      } else if (wasActive || !profiles.containsKey(activeProfile)) {
        await _setActiveProfile(profiles.keys.first, notify: false);
      }

      _safeNotifyListeners();
    } catch (error, stackTrace) {
      profiles
        ..clear()
        ..addAll(profilesBefore);
      activeProfile = activeBefore;
      Error.throwWithStackTrace(error, stackTrace);
    }
  }

  Future<void> setActiveProfile(String id) {
    return _enqueue(() => _setActiveProfile(id));
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
    activeProfile = targetId;

    try {
      await _setStringOption(_activeProfileKey, targetId);
    } catch (error, stackTrace) {
      activeProfile = previousActive;
      Error.throwWithStackTrace(error, stackTrace);
    }

    if (notify) {
      _safeNotifyListeners();
    }
  }

  Future<Map<String, Contact>> loadContacts(String id) async {
    final Map<String, Contact> contacts = <String, Contact>{};
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

      try {
        contacts[entry.key] = Contact.fromParts(
          id: entry.key,
          nickname: nickname,
          peerId: peerId,
          outputVolume: outputVolume,
        );
      } catch (error) {
        DebugConsole.warn('invalid contact format for ${entry.key}: $error');
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

    final Map<String, Map<String, dynamic>> contactsMap =
        <String, Map<String, dynamic>>{};

    for (final MapEntry<String, Contact> entry in profile.contacts.entries) {
      try {
        contactsMap[entry.key] = <String, dynamic>{
          'nickname': entry.value.nickname(),
          'peerId': entry.value.peerId(),
          'outputVolume': entry.value.outputVolume(),
        };
      } catch (error) {
        DebugConsole.warn('skipping contact ${entry.key} during save: $error');
      }
    }

    await _writeStorage(
      key: '$profileId-contacts',
      value: jsonEncode(contactsMap),
    );
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

  Future<void> _deleteProfileStorage(String id) async {
    await _deleteStorage('$id-keypair');
    await _deleteStorage('$id-peerId');
    await _deleteStorage('$id-contacts');
    await _deleteStorage('$id-rooms');
    await _deleteStorage('$id-nickname');
  }

  Future<void> _persistProfileIds() async {
    await _setStringListOption(
        _profilesKey, profiles.keys.toList(growable: false));
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
      activeProfile = fallbackId;
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
