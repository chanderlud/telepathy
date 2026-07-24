import 'dart:async';
import 'dart:convert';

import 'package:collection/collection.dart';
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

enum _ProfileRollbackPhase {
  prepared,
  indexRestored,
  activeRestored;

  static _ProfileRollbackPhase? parse(Object? value) {
    if (value is! String) {
      return null;
    }
    return values.firstWhereOrNull((phase) => phase.name == value);
  }
}

class _ProfileRollbackRecord {
  const _ProfileRollbackRecord({
    required this.deletedProfileId,
    required this.previousActiveProfileId,
    required this.phase,
  });

  static const int version = 1;

  final String deletedProfileId;
  final String previousActiveProfileId;
  final _ProfileRollbackPhase phase;

  _ProfileRollbackRecord withPhase(_ProfileRollbackPhase nextPhase) {
    return _ProfileRollbackRecord(
      deletedProfileId: deletedProfileId,
      previousActiveProfileId: previousActiveProfileId,
      phase: nextPhase,
    );
  }

  String encode() {
    return jsonEncode(<String, Object>{
      'version': version,
      'deletedProfileId': deletedProfileId,
      'previousActiveProfileId': previousActiveProfileId,
      'phase': phase.name,
    });
  }

  static _ProfileRollbackRecord? decode(String encoded) {
    try {
      final Object? decoded = jsonDecode(encoded);
      if (decoded is! Map) {
        return null;
      }
      final Map<String, dynamic> map = decoded.map<String, dynamic>(
        (dynamic key, dynamic value) =>
            MapEntry<String, dynamic>(key.toString(), value),
      );
      final Object? rawVersion = map['version'];
      final Object? rawDeletedProfileId = map['deletedProfileId'];
      final Object? rawPreviousActiveProfileId = map['previousActiveProfileId'];
      final _ProfileRollbackPhase? phase =
          _ProfileRollbackPhase.parse(map['phase']);
      if (rawVersion != version ||
          rawDeletedProfileId is! String ||
          rawDeletedProfileId.trim().isEmpty ||
          rawPreviousActiveProfileId is! String ||
          rawPreviousActiveProfileId.trim().isEmpty ||
          phase == null) {
        return null;
      }
      return _ProfileRollbackRecord(
        deletedProfileId: rawDeletedProfileId.trim(),
        previousActiveProfileId: rawPreviousActiveProfileId.trim(),
        phase: phase,
      );
    } catch (_) {
      return null;
    }
  }
}

class _ProfileRollbackJournal {
  const _ProfileRollbackJournal({
    required this.records,
    required this.unknownEntries,
  });

  final List<_ProfileRollbackRecord> records;
  final List<String> unknownEntries;

  bool get hasUnknownEntries => unknownEntries.isNotEmpty;

  List<String> encode() {
    return <String>[
      ...records.map((record) => record.encode()),
      ...unknownEntries,
    ];
  }
}

/// Steps of [ProfilesController.removeProfile] that can fail. Surfaced
/// through [ProfileDeletionException.phase] so the UI can render a
/// phase-specific message instead of always claiming the index update
/// succeeded.
enum ProfileDeletionPhase {
  /// The durable intent could not be persisted. The operation aborted
  /// before any destructive change.
  tombstoneWrite,

  /// `beginIdentitySwitch` failed; no slot was reserved.
  begin,

  /// The active-profile persistence (`activeProfile` pref) failed after
  /// begin. Pre-commit cancel path.
  activeIdPersist,

  /// The profile-index update (`profilesV2` pref) failed. Rolled back
  /// together with the active id and tombstone.
  indexWrite,

  /// `commitIdentitySwitch` failed. The backend rolled itself back;
  /// the frontend durably restores the `profilesV2` index and the
  /// `activeProfile` pref before clearing the deletion intent. If
  /// either rollback write cannot be confirmed, the structured
  /// write-ahead record remains so startup reconciles the
  /// still-present tombstone as a rollback (restore from intact
  /// storage) rather than a deletion (destroy storage).
  commit,

  /// The replacement profile could not be created (key generation,
  /// secure-storage write, or index persistence failed) during an
  /// active-profile deletion. NEVER classified as `storageCleanup`:
  /// the original active profile is still live and the user may still
  /// be using it, so a cleanup retry against it would erase the
  /// active private key. The deletion aborted before any durable
  /// destructive change or backend gate; any sole-profile preflight intent
  /// remains non-destructive while the original profile stays indexed.
  replacementCreate,

  /// The secure-storage records could not be deleted after the index
  /// update succeeded. The tombstone remains; startup retries.
  storageCleanup,
}

/// Thrown by [ProfilesController.removeProfile] when a deletion step
/// fails. Carries the [phase] that failed and the underlying [cause] so
/// the UI can render a phase-specific message and offer the right retry
/// action. [tombstonedForStartupRetry] is true when the failure left a
/// durable tombstone that the next startup will redrive.
class ProfileDeletionException implements Exception {
  const ProfileDeletionException({
    required this.phase,
    required this.cause,
    this.tombstonedForStartupRetry = false,
  });

  final ProfileDeletionPhase phase;
  final Object cause;
  final bool tombstonedForStartupRetry;

  @override
  String toString() => 'ProfileDeletionException(phase: $phase, cause: $cause, '
      'tombstonedForStartupRetry: $tombstonedForStartupRetry)';
}

class ProfilesController with ChangeNotifier {
  static const String _profilesKey = 'profilesV2';
  static const String _activeProfileKey = 'activeProfile';
  static const String _defaultProfileNickname = 'Default';
  static const String _unnamedProfileNickname = 'Unnamed Profile';
  static const double _minContactOutputVolumeDb = -15.0;
  static const double _maxContactOutputVolumeDb = 15.0;
  static const String _deletionTombstonesKey = 'profileDeletionTombstones';
  static const String _rollbackJournalKey = 'profileDeletionRollbackJournal';

  /// Legacy id-only rollback markers. Startup migrates these to the
  /// structured rollback journal before clearing this key.
  static const String _rollbackIntentsKey = 'profileRollbackIntents';

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

  /// True while a two-phase identity-switch transaction (profile switch or
  /// active-profile deletion) is in flight. All public profile-mutation
  /// methods reject re-entrant calls while this is true so the gate the
  /// backend holds across both layers cannot be raced from the frontend.
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

    // Honor the deletion journal BEFORE loading profiles so tombstoned
    // records cannot resurrect as profiles (or as switch targets) during
    // this startup. Tombstone semantics: "secure-storage cleanup is
    // pending for this id." The profile index remains the authoritative
    // statement of which profiles should exist, so a tombstoned id that
    // still appears in the index (e.g. a rollback restored it) is left
    // alone and the stale tombstone is cleared.
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
      await _initActiveProfile(defaultId);
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

      await _initActiveProfile(selectedId);
    }

    _initialized = true;
    _safeNotifyListeners();
  }

  /// This can still throw when the peer id is invalid because the historical API
  /// returns a non-null [Contact]. Use [tryAddContact] when taking user input.
  Contact addContact(String nickname, String peerId) {
    _rejectDuringTransaction('addContact');
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
    _rejectDuringTransaction('tryAddContact');
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

  void updateContact(
    Contact contact, {
    String? nickname,
    double? outputVolume,
  }) {
    _rejectDuringTransaction('updateContact');
    final Profile profile = _currentProfile();

    if (nickname != null) {
      contact.setNickname(nickname: nickname);
    }
    if (outputVolume != null) {
      contact.setOutputVolume(decibel: outputVolume);
    }

    _safeNotifyListeners();
    unawaited(_enqueue(() => _saveContactsFor(profile.id, notify: false)));
  }

  void removeContact(Contact contact) {
    _rejectDuringTransaction('removeContact');
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
    _rejectDuringTransaction('saveContacts');
    final String profileId = _currentProfile().id;
    _safeNotifyListeners();
    return _enqueue(() => _saveContactsFor(profileId, notify: false));
  }

  Room addRoom(String nickname, List<String> peerIds) {
    _rejectDuringTransaction('addRoom');
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
    _rejectDuringTransaction('tryAddRoom');
    try {
      return addRoom(nickname, peerIds);
    } catch (error) {
      DebugConsole.warn('room was not added: $error');
      return null;
    }
  }

  void updateRoom(Room room, {required String nickname}) {
    _rejectDuringTransaction('updateRoom');
    final Profile profile = _currentProfile();

    room.nickname = nickname;
    _safeNotifyListeners();
    unawaited(_enqueue(() => _saveRoomsFor(profile.id, notify: false)));
  }

  void removeRoom(Room room) {
    _rejectDuringTransaction('removeRoom');
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
    _rejectDuringTransaction('saveRooms');
    final String profileId = _currentProfile().id;
    _safeNotifyListeners();
    return _enqueue(() => _saveRoomsFor(profileId, notify: false));
  }

  Future<String> createProfile(String nickname) {
    _rejectDuringTransaction('createProfile');
    return _enqueue(() => _createProfile(nickname));
  }

  Future<String> _createProfile(String nickname, {bool notify = true}) async {
    final String cleanNickname =
        nickname.trim().isEmpty ? _unnamedProfileNickname : nickname;
    final String id = const Uuid().v4();

    // A generated private key must always have a durable cleanup path before
    // it can exist. Startup treats indexed tombstones as live profiles, so
    // this creation intent is non-destructive once the index is persisted.
    await _recordDeletionIntent(id);

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
      await _clearDeletionIntent(id);
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
    // Reject before queue submission: rejection, not serialization, is the
    // contract for an in-flight transaction. Queueing first would let a
    // sibling delete wait behind the switch only to throw when it reaches
    // the front, surprising the caller.
    _rejectDuringTransaction('removeProfile');
    return _enqueue(() => _removeProfile(id, telepathy: telepathy));
  }

  Future<void> _removeProfile(String id, {required Telepathy telepathy}) async {
    if (!profiles.containsKey(id)) {
      DebugConsole.warn('removeProfile called for unknown profile: $id');
      return;
    }

    final Map<String, Profile> previousProfiles =
        Map<String, Profile>.from(profiles);
    final String previousActive = _activeProfile;
    final bool wasActive = _activeProfile == id;

    // Non-active deletion touches neither the call slot nor the active
    // identity, so it stays out of the two-phase transaction. The
    // deletion journal is written BEFORE the index update or storage
    // delete so a crash in either step leaves a durable retry intent
    // instead of resurrecting the private key.
    if (!wasActive) {
      // Step 1: persist both write-ahead records. If either write fails,
      // abort BEFORE touching the index or storage so recovery never has to
      // infer whether a reported index failure committed its mutation.
      late _ProfileRollbackRecord rollbackRecord;
      try {
        await _recordDeletionIntent(id);
        rollbackRecord = await _recordPreparedRollback(
          deletedProfileId: id,
          previousActiveProfileId: previousActive,
        );
      } catch (error, stackTrace) {
        await _safeAbandonPreparedRollback(id);
        Error.throwWithStackTrace(
          ProfileDeletionException(
            phase: ProfileDeletionPhase.tombstoneWrite,
            cause: error,
          ),
          stackTrace,
        );
      }

      // Step 2: remove from the in-memory map and persist the index.
      // A reported failure may have committed the write, so restore the
      // in-memory map and use the phased rollback journal to durably restore
      // both the index and the previously active id before clearing protection.
      profiles.remove(id);
      try {
        await _persistProfileIds();
      } catch (error, stackTrace) {
        profiles
          ..clear()
          ..addAll(previousProfiles);
        final bool indexAlreadyRestored =
            await _persistedIndexMatches(previousProfiles.keys);
        await _recoverActiveDeletionPersistence(
          rollbackRecord,
          indexAlreadyRestored: indexAlreadyRestored,
        );
        Error.throwWithStackTrace(
          ProfileDeletionException(
            phase: ProfileDeletionPhase.indexWrite,
            cause: error,
          ),
          stackTrace,
        );
      }

      // Step 3: remove rollback authorization before physically deleting
      // secure-storage records. If the journal clear fails, the tombstone and
      // intact storage remain protected for startup recovery. After the clear,
      // the tombstone alone authorizes retrying storage cleanup.
      try {
        await _clearRollbackRecord(id, requireExisting: true);
        await _deleteProfileStorage(id);
        await _clearDeletionIntent(id);
      } catch (error, stackTrace) {
        DebugConsole.warn(
          'storage cleanup for non-active profile $id failed; '
          'tombstoned for startup retry: $error\n$stackTrace',
        );
        _safeNotifyListeners();
        Error.throwWithStackTrace(
          ProfileDeletionException(
            phase: ProfileDeletionPhase.storageCleanup,
            cause: error,
            tombstonedForStartupRetry: true,
          ),
          stackTrace,
        );
      }
      _safeNotifyListeners();
      return;
    }

    // Active deletion: the replacement identity must exist and be persisted
    // BEFORE the backend gate is acquired, per the two-phase contract. Pick
    // the first remaining profile or create a fresh default when this is the
    // sole profile.
    String? replacementId;
    Profile? replacement;
    for (final MapEntry<String, Profile> entry in profiles.entries) {
      if (entry.key != id) {
        replacementId = entry.key;
        replacement = entry.value;
        break;
      }
    }
    final bool createdReplacement = replacementId == null;
    late _ProfileRollbackRecord rollbackRecord;
    bool rollbackPreparedBeforeReplacement = false;
    if (createdReplacement) {
      // Establish both write-ahead records before generating replacement
      // keys. If either journal is unavailable, every layer remains untouched
      // and the backend transaction never starts.
      try {
        await _recordDeletionIntent(id);
        rollbackRecord = await _recordPreparedRollback(
          deletedProfileId: id,
          previousActiveProfileId: previousActive,
        );
        rollbackPreparedBeforeReplacement = true;
      } catch (error, stackTrace) {
        await _safeClearDeletionIntent(id);
        Error.throwWithStackTrace(
          ProfileDeletionException(
            phase: ProfileDeletionPhase.tombstoneWrite,
            cause: error,
          ),
          stackTrace,
        );
      }

      // NEVER classify a replacement-creation failure as `storageCleanup`.
      // The original active profile is still live (its storage records,
      // index entry, and active-id pref are all unchanged), and the user
      // may still be using it. A cleanup retry routed to this id would
      // erase the active private key. Wrap the failure in a distinct
      // phase so the UI cannot offer the destructive "Retry Cleanup"
      // button on it.
      try {
        replacementId =
            await _createProfile(_defaultProfileNickname, notify: false);
        replacement = profiles[replacementId]!;
      } catch (error, stackTrace) {
        await _safeAbandonPreparedRollback(id);
        Error.throwWithStackTrace(
          ProfileDeletionException(
            phase: ProfileDeletionPhase.replacementCreate,
            cause: error,
          ),
          stackTrace,
        );
      }
    }

    _isIdentitySwitchPending = true;
    _safeNotifyListeners();

    try {
      // begin validates the target payload (key length + contacts) BEFORE
      // reserving the slot, so a malformed replacement cannot wedge the
      // gate.
      final List<Contact> replacementSnapshot = replacement!.contacts.values
          .map((Contact c) => c.pubClone())
          .toList();
      try {
        await telepathy.beginIdentitySwitch(
          targetKey: replacement.keypair,
          targetContacts: replacementSnapshot,
        );
      } catch (error, stackTrace) {
        await _undoReplacementCreation(
          createdReplacement: createdReplacement,
          replacementId: replacementId,
        );
        if (rollbackPreparedBeforeReplacement) {
          await _safeAbandonPreparedRollback(id);
        }
        Error.throwWithStackTrace(
          ProfileDeletionException(
            phase: ProfileDeletionPhase.begin,
            cause: error,
          ),
          stackTrace,
        );
      }

      // Persist both write-ahead records before changing either preference
      // that the commit-failure path may need to restore. Sole-profile
      // deletion established them before replacement creation; other active
      // deletions establish them after begin so a write failure can cancel the
      // held backend gate without touching the profile index.
      if (!rollbackPreparedBeforeReplacement) {
        try {
          await _recordDeletionIntent(id);
          rollbackRecord = await _recordPreparedRollback(
            deletedProfileId: id,
            previousActiveProfileId: previousActive,
          );
        } catch (error, stackTrace) {
          await _safeClearDeletionIntent(id);
          await _undoReplacementCreation(
            createdReplacement: createdReplacement,
            replacementId: replacementId,
          );
          try {
            await telepathy.cancelIdentitySwitch();
          } catch (cancelError) {
            DebugConsole.warn(
              'cancelIdentitySwitch after intent write failure: $cancelError',
            );
          }
          Error.throwWithStackTrace(
            ProfileDeletionException(
              phase: ProfileDeletionPhase.tombstoneWrite,
              cause: error,
            ),
            stackTrace,
          );
        }
      }

      // Persist the target active profile while the backend gate is held.
      // On failure cancel BEFORE mutating Rust (per the two-phase contract).
      try {
        _activeProfile = replacementId;
        await _setStringOption(_activeProfileKey, replacementId);
      } catch (error, stackTrace) {
        _activeProfile = previousActive;
        await _recoverActiveDeletionPersistence(
          rollbackRecord,
          indexAlreadyRestored: true,
        );
        await _undoReplacementCreation(
          createdReplacement: createdReplacement,
          replacementId: replacementId,
        );
        try {
          await telepathy.cancelIdentitySwitch();
        } catch (cancelError) {
          DebugConsole.warn(
              'cancelIdentitySwitch after persist failure: $cancelError');
        }
        Error.throwWithStackTrace(
          ProfileDeletionException(
            phase: ProfileDeletionPhase.activeIdPersist,
            cause: error,
          ),
          stackTrace,
        );
      }

      // Remove the id from the index BEFORE committing to the backend.
      // Comment 1: every reversible persistence change (including the
      // index exclusion) must complete before commitIdentitySwitch. If
      // the index write fails, roll back the active id AND the tombstone
      // AND the in-memory map together and cancel so the backend stays
      // aligned on previousActive.
      profiles.remove(id);
      try {
        await _persistProfileIds();
      } catch (error, stackTrace) {
        profiles
          ..clear()
          ..addAll(previousProfiles);
        _activeProfile = previousActive;
        final bool indexAlreadyRestored =
            await _persistedIndexMatches(previousProfiles.keys);
        await _recoverActiveDeletionPersistence(
          rollbackRecord,
          indexAlreadyRestored: indexAlreadyRestored,
        );
        await _undoReplacementCreation(
          createdReplacement: createdReplacement,
          replacementId: replacementId,
          knownReplacement: replacement,
        );
        try {
          await telepathy.cancelIdentitySwitch();
        } catch (cancelError) {
          DebugConsole.warn(
              'cancelIdentitySwitch after index write failure: $cancelError');
        }
        Error.throwWithStackTrace(
          ProfileDeletionException(
            phase: ProfileDeletionPhase.indexWrite,
            cause: error,
          ),
          stackTrace,
        );
      }

      // Commit the replacement identity. After commit succeeds the
      // backend has forgotten the deleted profile's identity; per
      // Comment 1, NEVER restore _activeProfile to previousActive on
      // any subsequent failure. The replacement identity is
      // authoritative.
      //
      // On commit failure the backend rolls itself back to previousActive.
      // The prepared record already classifies every crash boundary as
      // recovery. Advance it only after each restored preference is durable;
      // a failed later write leaves the earliest safe phase in place.
      try {
        await telepathy.commitIdentitySwitch();
      } catch (error, stackTrace) {
        profiles
          ..clear()
          ..addAll(previousProfiles);
        _activeProfile = previousActive;
        await _recoverActiveDeletionPersistence(
          rollbackRecord,
          indexAlreadyRestored: false,
        );

        await _undoReplacementCreation(
          createdReplacement: createdReplacement,
          replacementId: replacementId,
          knownReplacement: replacement,
        );
        Error.throwWithStackTrace(
          ProfileDeletionException(
            phase: ProfileDeletionPhase.commit,
            cause: error,
          ),
          stackTrace,
        );
      }

      // Commit succeeded. Remove the recovery classification before deleting
      // storage. If that write fails, preserving the tombstone and intact
      // storage lets startup resolve the ambiguous state conservatively.
      try {
        await _clearRollbackRecord(id, requireExisting: true);
        await _deleteProfileStorage(id);
        await _clearDeletionIntent(id);
      } catch (error, stackTrace) {
        DebugConsole.warn(
          'storage cleanup for active profile $id failed; '
          'tombstoned for startup retry: $error\n$stackTrace',
        );
        _safeNotifyListeners();
        Error.throwWithStackTrace(
          ProfileDeletionException(
            phase: ProfileDeletionPhase.storageCleanup,
            cause: error,
            tombstonedForStartupRetry: true,
          ),
          stackTrace,
        );
      }
      _safeNotifyListeners();
    } finally {
      _isIdentitySwitchPending = false;
      _safeNotifyListeners();
    }
  }

  /// Retries the secure-storage cleanup for a previously-failed
  /// deletion. Used by the UI when [removeProfile] surfaced a
  /// `storageCleanup` failure that left `tombstonedForStartupRetry`
  /// set. Safe to call multiple times; idempotent.
  ///
  /// Safety: the retry is REJECTED without mutation unless ALL three
  /// invariants hold:
  ///   1. A durable tombstone for `id` exists in the deletion journal
  ///      (the journal is the authority that says cleanup is pending).
  ///   2. `id` is absent from the in-memory `profiles` map.
  ///   3. `id` is absent from the persisted `profilesV2` index.
  /// The check guards against a mis-routed retry (e.g. a
  /// replacement-creation failure mis-classified as cleanup by an
  /// older UI, or a stale request after the profile was already
  /// restored) erasing the active profile's private key while the
  /// user is still using it.
  ///
  /// Returns `true` when the cleanup succeeded and the tombstone was
  /// cleared. Returns `false` when the request was rejected (no
  /// mutation) OR the failure persists (the tombstone remains and
  /// startup will retry again).
  Future<bool> retryDeletionCleanup(String id) {
    return _enqueue(() => _retryDeletionCleanup(id));
  }

  Future<bool> _retryDeletionCleanup(String id) async {
    // Invariant 1: a durable tombstone must exist. Without it, no
    // cleanup is pending — the request is mis-routed and the id may
    // still be live.
    final List<String> tombstones = _dedupe(
      await _getStringListOption(_deletionTombstonesKey) ?? const <String>[],
    );
    if (!tombstones.contains(id)) {
      DebugConsole.warn(
        'retryDeletionCleanup rejected for $id: no durable tombstone; '
        'the profile may still be live, refusing to delete storage',
      );
      return false;
    }
    try {
      final _ProfileRollbackJournal journal =
          await _loadAndMigrateRollbackJournal();
      final List<String> legacyRollbacks = _dedupe(
        await _getStringListOption(_rollbackIntentsKey) ?? const <String>[],
      );
      if (journal.hasUnknownEntries ||
          journal.records.any((record) => record.deletedProfileId == id) ||
          legacyRollbacks.contains(id)) {
        DebugConsole.warn(
          'retryDeletionCleanup rejected for $id: rollback recovery state '
          'protects this profile; refusing to delete storage',
        );
        return false;
      }
    } catch (error, stackTrace) {
      DebugConsole.warn(
        'retryDeletionCleanup rejected for $id: rollback recovery state '
        'could not be classified safely: $error\n$stackTrace',
      );
      return false;
    }
    // Invariant 2: the id must not be in the in-memory profiles map.
    if (profiles.containsKey(id)) {
      DebugConsole.warn(
        'retryDeletionCleanup rejected for $id: id is still in the '
        'in-memory profiles map; refusing to delete storage for a '
        'live profile',
      );
      return false;
    }
    // Invariant 3: the id must not be in the persisted profilesV2
    // index. A tombstone + index entry means a rollback restored the
    // profile after the tombstone was written; deleting storage would
    // destroy the active private key.
    final List<String> persistedIndex = _dedupe(
      await _getStringListOption(_profilesKey) ?? const <String>[],
    );
    if (persistedIndex.contains(id)) {
      DebugConsole.warn(
        'retryDeletionCleanup rejected for $id: id is still in the '
        'persisted profilesV2 index; refusing to delete storage for a '
        'live profile',
      );
      return false;
    }
    try {
      await _deleteProfileStorage(id);
      await _clearDeletionIntent(id);
      _safeNotifyListeners();
      return true;
    } catch (error, stackTrace) {
      DebugConsole.warn(
        'manual cleanup retry for $id failed; tombstone remains: '
        '$error\n$stackTrace',
      );
      return false;
    }
  }

  Future<void> _undoReplacementCreation({
    required bool createdReplacement,
    required String? replacementId,
    Profile? knownReplacement,
  }) async {
    if (!createdReplacement || replacementId == null) {
      return;
    }
    final Profile? replacement =
        profiles.remove(replacementId) ?? knownReplacement;

    bool intentRecorded = false;
    try {
      await _recordDeletionIntent(replacementId);
      intentRecorded = true;
    } catch (error, stackTrace) {
      DebugConsole.warn(
        'failed to tombstone replacement $replacementId during rollback; '
        'continuing index and storage cleanup without a durable retry path: '
        '$error\n$stackTrace',
      );
    }

    bool indexExcluded = false;
    try {
      await _persistProfileIds();
      indexExcluded = true;
    } catch (error, stackTrace) {
      if (replacement != null) {
        profiles[replacementId] = replacement;
      }
      DebugConsole.warn(
        'failed to exclude replacement $replacementId from the profile index '
        'during rollback; retaining the indexed profile and its storage: '
        '$error\n$stackTrace',
      );
      return;
    }

    bool storageDeleted = false;
    try {
      await _deleteProfileStorage(replacementId);
      storageDeleted = true;
    } catch (error, stackTrace) {
      DebugConsole.warn(
        'replacement $replacementId storage cleanup failed during rollback; '
        'durable intent retained when available: $error\n$stackTrace',
      );
    }

    if (intentRecorded && indexExcluded && storageDeleted) {
      await _safeClearDeletionIntent(replacementId);
    }
  }

  /// Switches the active profile through the two-phase identity-switch
  /// transaction. Validates the target payload + acquires the backend
  /// `IdentitySwitch` gate, persists the target active profile, then
  /// commits the new signing key and contact snapshot. On any failure
  /// restores the frontend to its previous state and either cancels
  /// (pre-commit) or relies on Rust's internal rollback (post-commit).
  ///
  /// This is the ONLY public runtime path that mutates the active profile:
  /// the previous `setActiveProfile` setter bypassed the backend entirely,
  /// so the frontend could end up pointing at a profile whose signing key
  /// the backend had never installed. Startup selection uses the private
  /// [`_setActiveProfile`] / [`_initActiveProfile`] helpers instead.
  Future<void> switchActiveProfile(
    String id, {
    required Telepathy telepathy,
  }) {
    // Reject before queue submission: rejection, not serialization, is the
    // contract for an in-flight transaction.
    _rejectDuringTransaction('switchActiveProfile');
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

    final String previousActive = _activeProfile;
    final Profile target = profiles[id]!;

    _isIdentitySwitchPending = true;
    _safeNotifyListeners();

    try {
      final List<Contact> snapshot =
          target.contacts.values.map((Contact c) => c.pubClone()).toList();

      // begin validates the target keypair + contact snapshot BEFORE
      // reserving the slot. A malformed target returns an error here
      // without wedging the slot, satisfying the contract that no
      // validation failure can leave the gate reserved.
      try {
        await telepathy.beginIdentitySwitch(
          targetKey: target.keypair,
          targetContacts: snapshot,
        );
      } catch (error, stackTrace) {
        Error.throwWithStackTrace(error, stackTrace);
      }

      // Persist the target active profile while the backend gate is held.
      // On failure cancel BEFORE mutating Rust (per the two-phase contract).
      try {
        _activeProfile = id;
        await _setStringOption(_activeProfileKey, id);
      } catch (error, stackTrace) {
        _activeProfile = previousActive;
        try {
          await telepathy.cancelIdentitySwitch();
        } catch (cancelError) {
          DebugConsole.warn(
              'cancelIdentitySwitch after persist failure: $cancelError');
        }
        Error.throwWithStackTrace(error, stackTrace);
      }

      // Parameterless commit: the target identity + contact snapshot were
      // validated and stashed at begin. On failure the backend rolls
      // itself back; we restore the frontend active profile to match.
      try {
        await telepathy.commitIdentitySwitch();
      } catch (error, stackTrace) {
        _activeProfile = previousActive;
        try {
          await _setStringOption(_activeProfileKey, previousActive);
        } catch (rollbackError) {
          DebugConsole.warn(
              'failed to persist active profile rollback: $rollbackError');
        }
        Error.throwWithStackTrace(error, stackTrace);
      }
    } finally {
      _isIdentitySwitchPending = false;
      _safeNotifyListeners();
    }
  }

  void _rejectDuringTransaction(String op) {
    if (isIdentitySwitchPending) {
      throw StateError('cannot $op while identity switch is pending');
    }
  }

  /// Initialization-only helper: selects the active profile from persisted
  /// state without touching the Rust backend. The backend's identity is
  /// installed separately at startup (see `main.dart`'s `setIdentity` call),
  /// so this helper only needs to align the frontend's `activeProfile`
  /// field with what was persisted. Runtime changes MUST NOT use this path
  /// — they would desynchronize the frontend from the backend's signing
  /// key. Use [switchActiveProfile] for runtime mutations.
  Future<void> _initActiveProfile(String id, {bool notify = false}) async {
    return _setActiveProfile(id, notify: notify);
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
      keypair: keyBytes is Uint8List ? keyBytes : Uint8List.fromList(keyBytes),
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

  Future<void> _cleanupFailedProfileCreation(String id) async {
    profiles.remove(id);

    bool intentRecorded = false;
    try {
      await _recordDeletionIntent(id);
      intentRecorded = true;
    } catch (error, stackTrace) {
      DebugConsole.warn(
        'failed to confirm deletion intent for profile $id after create '
        'error; continuing index and storage cleanup: $error\n$stackTrace',
      );
    }

    bool indexExcluded = false;
    try {
      await _persistProfileIds();
      indexExcluded = true;
    } catch (error, stackTrace) {
      DebugConsole.warn(
        'failed to exclude profile $id from the profile index after create '
        'error; retaining its storage: $error\n$stackTrace',
      );
      return;
    }

    bool storageDeleted = false;
    try {
      await _deleteProfileStorage(id);
      storageDeleted = true;
    } catch (error, stackTrace) {
      DebugConsole.warn(
        'failed to delete profile $id storage after create error; durable '
        'intent retained when available: $error\n$stackTrace',
      );
    }

    if (intentRecorded && indexExcluded && storageDeleted) {
      await _safeClearDeletionIntent(id);
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

  Future<void> _persistProfileIds() async {
    await _setStringListOption(
        _profilesKey, profiles.keys.toList(growable: false));
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

  /// Best-effort clear used inside rollback paths where propagating the
  /// failure would mask the original error. Logs the failure so it is
  /// observable; startup's [_retryTombstonedDeletions] reconciles any
  /// stale tombstone whose id is still in the index.
  Future<void> _safeClearDeletionIntent(String id) async {
    try {
      await _clearDeletionIntent(id);
    } catch (error, stackTrace) {
      DebugConsole.warn(
        'failed to clear deletion intent for $id during rollback: '
        '$error\n$stackTrace',
      );
    }
  }

  Future<_ProfileRollbackJournal> _readRollbackJournal() async {
    final List<String> encodedRecords = await _getStringListOption(
          _rollbackJournalKey,
        ) ??
        const <String>[];
    final List<_ProfileRollbackRecord> records = <_ProfileRollbackRecord>[];
    final List<String> unknownEntries = <String>[];

    for (final String encoded in encodedRecords) {
      final _ProfileRollbackRecord? record =
          _ProfileRollbackRecord.decode(encoded);
      if (record == null) {
        unknownEntries.add(encoded);
        DebugConsole.warn(
          'unknown profile rollback journal entry retained without cleanup '
          'authorization: $encoded',
        );
        continue;
      }
      final int existingIndex = records.indexWhere(
        (existing) => existing.deletedProfileId == record.deletedProfileId,
      );
      if (existingIndex == -1) {
        records.add(record);
        continue;
      }
      final _ProfileRollbackRecord existing = records[existingIndex];
      if (existing.previousActiveProfileId != record.previousActiveProfileId) {
        unknownEntries.add(encoded);
        DebugConsole.warn(
          'conflicting profile rollback journal entry retained for '
          '${record.deletedProfileId}',
        );
        continue;
      }
      if (record.phase.index < existing.phase.index) {
        records[existingIndex] = record;
      }
    }

    return _ProfileRollbackJournal(
      records: records,
      unknownEntries: unknownEntries,
    );
  }

  Future<_ProfileRollbackJournal> _loadAndMigrateRollbackJournal() async {
    final _ProfileRollbackJournal journal = await _readRollbackJournal();
    final List<String> legacyRollbacks = _dedupe(
      await _getStringListOption(_rollbackIntentsKey) ?? const <String>[],
    );
    if (legacyRollbacks.isEmpty) {
      return journal;
    }

    final List<_ProfileRollbackRecord> migratedRecords =
        List<_ProfileRollbackRecord>.from(journal.records);
    for (final String id in legacyRollbacks) {
      if (migratedRecords.any((record) => record.deletedProfileId == id)) {
        continue;
      }
      migratedRecords.add(
        _ProfileRollbackRecord(
          deletedProfileId: id,
          previousActiveProfileId: id,
          phase: _ProfileRollbackPhase.prepared,
        ),
      );
    }
    final _ProfileRollbackJournal migrated = _ProfileRollbackJournal(
      records: migratedRecords,
      unknownEntries: journal.unknownEntries,
    );

    // The structured journal must be durable before legacy protection is
    // removed. A failed legacy clear leaves both copies, which is safe and
    // idempotent on the next load.
    await _persistRollbackJournal(migrated);
    try {
      await _setStringListOption(_rollbackIntentsKey, const <String>[]);
    } catch (error, stackTrace) {
      DebugConsole.warn(
        'legacy profile rollback intents remain after structured migration: '
        '$error\n$stackTrace',
      );
    }
    return migrated;
  }

  Future<void> _persistRollbackJournal(
    _ProfileRollbackJournal journal,
  ) async {
    await _setStringListOption(_rollbackJournalKey, journal.encode());
  }

  Future<_ProfileRollbackRecord> _recordPreparedRollback({
    required String deletedProfileId,
    required String previousActiveProfileId,
  }) async {
    final _ProfileRollbackJournal journal =
        await _loadAndMigrateRollbackJournal();
    if (journal.hasUnknownEntries) {
      throw StateError(
        'cannot prepare profile deletion while rollback journal contains '
        'unknown entries',
      );
    }
    if (journal.records
        .any((record) => record.deletedProfileId == deletedProfileId)) {
      throw StateError(
        'profile $deletedProfileId already has pending rollback recovery',
      );
    }
    final _ProfileRollbackRecord record = _ProfileRollbackRecord(
      deletedProfileId: deletedProfileId,
      previousActiveProfileId: previousActiveProfileId,
      phase: _ProfileRollbackPhase.prepared,
    );
    await _persistRollbackJournal(
      _ProfileRollbackJournal(
        records: <_ProfileRollbackRecord>[...journal.records, record],
        unknownEntries: journal.unknownEntries,
      ),
    );
    return record;
  }

  Future<_ProfileRollbackRecord> _transitionRollbackRecord(
    _ProfileRollbackRecord expected,
    _ProfileRollbackPhase nextPhase,
  ) async {
    final _ProfileRollbackJournal journal = await _readRollbackJournal();
    final int recordIndex = journal.records.indexWhere(
      (record) => record.deletedProfileId == expected.deletedProfileId,
    );
    if (recordIndex == -1) {
      throw StateError(
        'missing rollback record for ${expected.deletedProfileId}',
      );
    }
    final _ProfileRollbackRecord current = journal.records[recordIndex];
    if (current.previousActiveProfileId != expected.previousActiveProfileId) {
      throw StateError(
        'rollback record changed for ${expected.deletedProfileId}',
      );
    }
    if (current.phase.index >= nextPhase.index) {
      return current;
    }
    if (nextPhase.index != current.phase.index + 1) {
      throw StateError(
        'invalid rollback phase transition from ${current.phase.name} to '
        '${nextPhase.name}',
      );
    }
    final _ProfileRollbackRecord updated = current.withPhase(nextPhase);
    final List<_ProfileRollbackRecord> records =
        List<_ProfileRollbackRecord>.from(journal.records)
          ..[recordIndex] = updated;
    await _persistRollbackJournal(
      _ProfileRollbackJournal(
        records: records,
        unknownEntries: journal.unknownEntries,
      ),
    );
    return updated;
  }

  Future<void> _clearRollbackRecord(
    String id, {
    bool requireExisting = false,
  }) async {
    final _ProfileRollbackJournal journal =
        await _loadAndMigrateRollbackJournal();
    final List<String> legacyRollbacks = _dedupe(
      await _getStringListOption(_rollbackIntentsKey) ?? const <String>[],
    );
    if (journal.hasUnknownEntries || legacyRollbacks.contains(id)) {
      throw StateError(
        'rollback state for $id remains ambiguous; refusing to clear '
        'recovery protection',
      );
    }
    if (!journal.records.any((record) => record.deletedProfileId == id)) {
      if (requireExisting) {
        throw StateError(
          'prepared rollback record for $id is missing; refusing storage '
          'cleanup',
        );
      }
      return;
    }
    await _persistRollbackJournal(
      _ProfileRollbackJournal(
        records: journal.records
            .where((record) => record.deletedProfileId != id)
            .toList(),
        unknownEntries: journal.unknownEntries,
      ),
    );
  }

  Future<void> _safeAbandonPreparedRollback(String id) async {
    try {
      await _clearRollbackRecord(id);
      await _clearDeletionIntent(id);
    } catch (error, stackTrace) {
      DebugConsole.warn(
        'failed to abandon prepared rollback for $id; recovery protection '
        'remains: $error\n$stackTrace',
      );
    }
  }

  Future<void> _recoverActiveDeletionPersistence(
    _ProfileRollbackRecord preparedRecord, {
    required bool indexAlreadyRestored,
  }) async {
    try {
      _ProfileRollbackRecord current = preparedRecord;
      if (!indexAlreadyRestored) {
        await _persistProfileIds();
      }
      current = await _transitionRollbackRecord(
        current,
        _ProfileRollbackPhase.indexRestored,
      );
      await _setStringOption(
        _activeProfileKey,
        current.previousActiveProfileId,
      );
      current = await _transitionRollbackRecord(
        current,
        _ProfileRollbackPhase.activeRestored,
      );
      await _clearDeletionIntent(current.deletedProfileId);
      await _clearRollbackRecord(current.deletedProfileId);
    } catch (error, stackTrace) {
      DebugConsole.warn(
        'active deletion rollback remains protected for '
        '${preparedRecord.deletedProfileId}: $error\n$stackTrace',
      );
    }
  }

  Future<bool> _persistedIndexMatches(Iterable<String> expectedIds) async {
    try {
      final Set<String> persisted = _dedupe(
        await _getStringListOption(_profilesKey) ?? const <String>[],
      ).toSet();
      return const SetEquality<String>().equals(
        persisted,
        expectedIds.toSet(),
      );
    } catch (error, stackTrace) {
      DebugConsole.warn(
        'could not confirm restored profile index: $error\n$stackTrace',
      );
      return false;
    }
  }

  /// Redrives secure-storage cleanup for any tombstoned profile ids at
  /// startup. Runs BEFORE profile loading so tombstoned records cannot
  /// resurrect as profiles or switch targets during this startup.
  ///
  /// Structured and legacy rollback records take precedence over cleanup.
  /// Every recoverable missing id is merged into one cumulative index write,
  /// then each record's prior active id is restored before its protection is
  /// cleared. Unknown records keep every otherwise-destructive tombstone for
  /// another startup rather than authorizing storage deletion.
  Future<void> _retryTombstonedDeletions() async {
    final _ProfileRollbackJournal journal =
        await _loadAndMigrateRollbackJournal();
    final List<String> index = _dedupe(
      await _getStringListOption(_profilesKey) ?? const <String>[],
    );
    final Set<String> indexedIds = index.toSet();
    final Set<String> restorableIds = <String>{};
    bool indexChanged = false;

    for (final _ProfileRollbackRecord record in journal.records) {
      if (indexedIds.contains(record.deletedProfileId)) {
        restorableIds.add(record.deletedProfileId);
        continue;
      }
      final Profile? profile = await _loadProfile(record.deletedProfileId);
      if (profile == null) {
        DebugConsole.warn(
          'startup cannot restore rollback profile '
          '${record.deletedProfileId}; storage records are missing or '
          'invalid, preserving all recovery state',
        );
        continue;
      }
      indexedIds.add(record.deletedProfileId);
      index.add(record.deletedProfileId);
      restorableIds.add(record.deletedProfileId);
      indexChanged = true;
    }

    if (indexChanged) {
      try {
        await _setStringListOption(_profilesKey, index);
      } catch (error, stackTrace) {
        DebugConsole.warn(
          'startup could not durably restore cumulative rollback index; '
          'preserving tombstones and journal entries: $error\n$stackTrace',
        );
        return;
      }
    }

    for (final _ProfileRollbackRecord record in journal.records) {
      if (!restorableIds.contains(record.deletedProfileId)) {
        continue;
      }
      try {
        _ProfileRollbackRecord current = record;
        if (current.phase == _ProfileRollbackPhase.prepared) {
          current = await _transitionRollbackRecord(
            current,
            _ProfileRollbackPhase.indexRestored,
          );
        }
        await _setStringOption(
          _activeProfileKey,
          current.previousActiveProfileId,
        );
        if (current.phase == _ProfileRollbackPhase.indexRestored) {
          current = await _transitionRollbackRecord(
            current,
            _ProfileRollbackPhase.activeRestored,
          );
        }
        await _clearDeletionIntent(current.deletedProfileId);
        await _clearRollbackRecord(current.deletedProfileId);
        DebugConsole.info(
          'startup restored rollback profile ${current.deletedProfileId} '
          'and prior active id ${current.previousActiveProfileId}',
        );
      } catch (error, stackTrace) {
        DebugConsole.warn(
          'startup rollback recovery remains pending for '
          '${record.deletedProfileId}: $error\n$stackTrace',
        );
        break;
      }
    }

    final _ProfileRollbackJournal remainingJournal =
        await _loadAndMigrateRollbackJournal();
    final List<String> remainingLegacyRollbacks = _dedupe(
      await _getStringListOption(_rollbackIntentsKey) ?? const <String>[],
    );
    final Set<String> protectedIds = <String>{
      ...remainingJournal.records.map((record) => record.deletedProfileId),
      ...remainingLegacyRollbacks,
    };
    final List<String> tombstones = _dedupe(
      await _getStringListOption(_deletionTombstonesKey) ?? const <String>[],
    );
    final List<String> remainingTombstones = <String>[];
    for (final String id in tombstones) {
      if (protectedIds.contains(id) || remainingJournal.hasUnknownEntries) {
        remainingTombstones.add(id);
        continue;
      }
      if (indexedIds.contains(id)) {
        DebugConsole.info(
          'startup clearing stale tombstone for $id; '
          'profile is still in the index (likely restored by a rollback)',
        );
        continue;
      }
      try {
        await _deleteProfileStorage(id);
        DebugConsole.info(
          'startup re-drove storage cleanup for tombstoned profile $id',
        );
      } catch (error, stackTrace) {
        DebugConsole.warn(
          'startup storage cleanup retry failed for profile $id; '
          'will retry again on next startup: $error\n$stackTrace',
        );
        remainingTombstones.add(id);
      }
    }
    await _setStringListOption(
      _deletionTombstonesKey,
      remainingTombstones,
    );
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
