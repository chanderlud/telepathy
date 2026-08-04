import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';

import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:shared_preferences_platform_interface/in_memory_shared_preferences_async.dart';
import 'package:shared_preferences_platform_interface/shared_preferences_async_platform_interface.dart';
import 'package:telepathy/controllers/profiles_controller.dart';
import 'package:telepathy/core/rust/frb_generated.dart';
import 'package:telepathy/core/rust/flutter.dart';
import 'package:telepathy/core/rust/lib.dart';
import 'package:telepathy/core/rust/types.dart';

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

  test('prepare rejection leaves active profile unchanged', () async {
    final fixture = await _fixture();
    fixture.telepathy.prepareError = StateError('invalid target');

    await expectLater(
      fixture.controller
          .switchActiveProfile('beta', telepathy: fixture.telepathy),
      throwsStateError,
    );

    expect(fixture.controller.activeProfile, 'alpha');
    expect(fixture.controller.isIdentitySwitchPending, isFalse);
    expect(await fixture.options.getString('activeProfile'), 'alpha');
    expect(fixture.telepathy.commits, 0);
  });

  test('invalid persisted keypairs are excluded and index is repaired',
      () async {
    final fixture = await _fixture();
    await fixture.storage.write(key: 'alpha-keypair', value: 'not-base64');
    await fixture.storage.write(
      key: 'beta-keypair',
      value: base64Encode(List<int>.filled(31, 2)),
    );
    final restarted = ProfilesController(
      storage: fixture.storage,
      options: fixture.options,
      roomHasher: _roomHash,
    );

    await restarted.init(const <String>[]);

    expect(restarted.profiles.keys, const <String>['gamma']);
    expect(restarted.activeProfile, 'gamma');
    expect(
      await fixture.options.getStringList('profilesV2'),
      const <String>['gamma'],
    );
    expect(await fixture.storage.read(key: 'alpha-keypair'), 'not-base64');
    expect(await fixture.storage.read(key: 'beta-keypair'), isNotNull);
  });

  Future<void> verifyDirectInvitationMigration() async {
    final fixture = await _fixture();
    final String aliceInvitation = _canonicalInvitation('alice-peer');
    final String otherPeerInvitation = _canonicalInvitation('other-peer');
    await fixture.storage.write(
      key: 'alpha-contacts',
      value: jsonEncode(<String, dynamic>{
        'alice-contact': <String, dynamic>{
          'nickname': 'Alice Nguyen',
          'peerId': 'alice-peer',
          'outputVolume': -4.5,
          'isDirect': true,
          'directInvitation': aliceInvitation,
        },
        'ben-contact': <String, dynamic>{
          'nickname': 'Ben Ortiz',
          'peerId': 'ben-peer',
          'outputVolume': 2.25,
          'isDirect': true,
          'directConnectionString': _legacyAddresses,
        },
        'carol-contact': <String, dynamic>{
          'nickname': 'Carol Smith',
          'peerId': 'carol-peer',
          'outputVolume': -7.0,
          'isDirect': true,
          'directInvitation': 'tp1:not-valid-base64',
        },
        'diego-contact': <String, dynamic>{
          'nickname': 'Diego Morales',
          'peerId': 'diego-peer',
          'outputVolume': 1.5,
          'isDirect': true,
          'directInvitation': <String, dynamic>{'unexpected': 'object'},
        },
        'erin-contact': <String, dynamic>{
          'nickname': 'Erin Chen',
          'peerId': 'erin-peer',
          'outputVolume': 3.0,
          'isDirect': true,
          'directInvitation': otherPeerInvitation,
        },
        'fatima-contact': <String, dynamic>{
          'nickname': 'Fatima Zahra',
          'peerId': 'fatima-peer',
          'outputVolume': 6.75,
          'isDirect': false,
        },
      }),
    );
    final restarted = ProfilesController(
      storage: fixture.storage,
      options: fixture.options,
      roomHasher: _roomHash,
    );

    await restarted.init(const <String>[]);

    final Map<String, Contact> contacts = restarted.profiles['alpha']!.contacts;
    expect(contacts, hasLength(6));
    expect(contacts['alice-contact']!.directInvitation(), aliceInvitation);
    expect(contacts['alice-contact']!.isDirect(), isTrue);
    expect(
      contacts['ben-contact']!.directInvitation(),
      _canonicalInvitation('ben-peer'),
    );
    expect(contacts['ben-contact']!.isDirect(), isTrue);
    for (final String id in <String>[
      'carol-contact',
      'diego-contact',
      'erin-contact',
    ]) {
      expect(contacts[id]!.directInvitation(), isNull);
      expect(contacts[id]!.isDirect(), isFalse);
    }
    expect(contacts['fatima-contact']!.nickname(), 'Fatima Zahra');
    expect(contacts['fatima-contact']!.outputVolume(), 6.75);

    final Map<String, dynamic> repaired = jsonDecode(
      (await fixture.storage.read(key: 'alpha-contacts'))!,
    ) as Map<String, dynamic>;
    expect(jsonEncode(repaired), isNot(contains('directConnectionString')));
    expect(repaired['ben-contact']['directInvitation'],
        _canonicalInvitation('ben-peer'));
    expect(repaired['carol-contact']['isDirect'], isFalse);
    expect(repaired['diego-contact']['isDirect'], isFalse);
    expect(repaired['erin-contact']['isDirect'], isFalse);
    expect(repaired['fatima-contact']['nickname'], 'Fatima Zahra');
    expect(repaired['fatima-contact']['outputVolume'], 6.75);
  }

  Future<void> verifyInteractiveInvitationAddIsAtomic() async {
    final fixture = await _fixture();

    expect(
      () => fixture.controller.addContact(
        'Grace Kim',
        'grace-peer',
        directInvitation: _canonicalInvitation('different-peer'),
      ),
      throwsA(isA<DartError>()),
    );
    expect(fixture.controller.contacts, isEmpty);

    final Contact contact = fixture.controller.addContact(
      'Grace Kim',
      'grace-peer',
      directInvitation: _canonicalInvitation('grace-peer'),
    );
    expect(fixture.controller.contacts, hasLength(1));
    expect(fixture.controller.contacts.values.single, same(contact));
    expect(contact.directInvitation(), _canonicalInvitation('grace-peer'));
    expect(contact.isDirect(), isTrue);
  }

  test('persistence failure disposes prepared token and keeps active profile',
      () async {
    final fixture = await _fixture(options: _ThrowingPreferences());
    (fixture.options as _ThrowingPreferences).failures.failActiveWrite = true;

    await expectLater(
      fixture.controller
          .switchActiveProfile('beta', telepathy: fixture.telepathy),
      throwsA(isA<Object>()),
    );

    expect(fixture.controller.activeProfile, 'alpha');
    expect(await fixture.options.getString('activeProfile'), 'alpha');
    expect(fixture.telepathy.disposals, 1);
    expect(fixture.telepathy.commits, 0);
  });

  test('confirmed persistence selects memory target before synchronous commit',
      () async {
    final fixture = await _fixture();
    fixture.telepathy.onCommit = () {
      expect(fixture.controller.activeProfile, 'beta');
    };

    await fixture.controller
        .switchActiveProfile('beta', telepathy: fixture.telepathy);

    expect(await fixture.options.getString('activeProfile'), 'beta');
    expect(fixture.controller.activeProfile, 'beta');
    expect(fixture.telepathy.commits, 1);
  });

  test('switch remains pending until runtime-ready commit completes', () async {
    final fixture = await _fixture();
    fixture.telepathy.pauseCommit();
    var completed = false;
    final switching = fixture.controller
        .switchActiveProfile('beta', telepathy: fixture.telepathy)
        .then((_) => completed = true);

    await fixture.telepathy.commitEntered.future;

    expect(fixture.controller.isIdentitySwitchPending, isTrue);
    expect(completed, isFalse);

    fixture.telepathy.releaseCommit();
    await switching;

    expect(fixture.controller.isIdentitySwitchPending, isFalse);
    expect(fixture.controller.activeProfile, 'beta');
    expect(await fixture.options.getString('activeProfile'), 'beta');
  });

  test('terminal commit failure keeps durable target and clears pending',
      () async {
    final fixture = await _fixture();
    fixture.telepathy.pauseCommit();
    final switching = fixture.controller
        .switchActiveProfile('beta', telepathy: fixture.telepathy);

    await fixture.telepathy.commitEntered.future;
    fixture.telepathy.commitError = StateError('runtime not ready');
    fixture.telepathy.releaseCommit();

    await expectLater(switching, throwsStateError);

    expect(fixture.controller.isIdentitySwitchPending, isFalse);
    expect(fixture.controller.activeProfile, 'beta');
    expect(await fixture.options.getString('activeProfile'), 'beta');
  });

  test('queued switches converge on latest target', () async {
    final fixture = await _fixture();
    fixture.telepathy.pausePrepare();
    final first = fixture.controller
        .switchActiveProfile('beta', telepathy: fixture.telepathy);
    final second = fixture.controller
        .switchActiveProfile('gamma', telepathy: fixture.telepathy);
    await fixture.telepathy.prepareEntered.future;
    fixture.telepathy.releasePrepare();
    await Future.wait(<Future<void>>[first, second]);

    expect(fixture.controller.activeProfile, 'gamma');
    expect(fixture.telepathy.commits, 2);
  });

  test('active deletion switches replacement before index removal', () async {
    final fixture = await _fixture();
    fixture.telepathy.onCommit = () {
      expect(fixture.controller.activeProfile, 'beta');
      expect(fixture.controller.profiles, contains('alpha'));
    };

    await fixture.controller
        .removeProfile('alpha', telepathy: fixture.telepathy);

    expect(fixture.controller.activeProfile, 'beta');
    expect(fixture.controller.profiles, isNot(contains('alpha')));
  });

  test('index persistence failure preserves profile map and storage', () async {
    final fixture = await _fixture(options: _ThrowingPreferences());
    (fixture.options as _ThrowingPreferences).failures.failIndexWrite = true;
    final keyBefore = await fixture.storage.read(key: 'gamma-keypair');

    await expectLater(
      fixture.controller.removeProfile('gamma', telepathy: fixture.telepathy),
      throwsA(isA<Object>()),
    );

    expect(fixture.controller.profiles, contains('gamma'));
    expect(await fixture.storage.read(key: 'gamma-keypair'), keyBefore);
  });

  test('failed creation cleanup is tombstoned and retried at startup',
      () async {
    final storage = _ThrowingStorage();
    final fixture = await _fixture(
      options: _ThrowingPreferences(),
      storage: storage,
    );
    final preferences = fixture.options as _ThrowingPreferences
      ..failures.failIndexWrite = true;
    storage.failDeletes = true;

    await expectLater(
      fixture.controller.createProfile('Cleanup Retry'),
      throwsA(isA<Object>()),
    );

    final tombstones =
        await preferences.getStringList('profileDeletionTombstones');
    expect(tombstones, hasLength(1));
    final id = tombstones!.single;
    expect(fixture.controller.profiles, isNot(contains(id)));
    expect(await preferences.getStringList('profilesV2'),
        const <String>['alpha', 'beta', 'gamma']);
    expect(await storage.read(key: '$id-keypair'), isNull);
    await storage.write(key: '$id-keypair', value: 'pending-cleanup');

    storage.failDeletes = false;
    final restarted = ProfilesController(
      storage: storage,
      options: preferences,
      roomHasher: _roomHash,
    );
    await restarted.init(const <String>[]);

    expect(await storage.read(key: '$id-keypair'), isNull);
    expect(storage.deleteCalls, greaterThan(0));
    expect(
        await preferences.getStringList('profileDeletionTombstones'), isEmpty);
  });

  test('cleanup failure attempts later keys and tombstone retry clears it',
      () async {
    final storage = _ThrowingStorage();
    final fixture = await _fixture(storage: storage);
    storage.failDeleteKeys.add('gamma-keypair');

    await expectLater(
      fixture.controller.removeProfile('gamma', telepathy: fixture.telepathy),
      throwsStateError,
    );

    expect(
      storage.deletedKeys,
      const <String>[
        'gamma-keypair',
        'gamma-peerId',
        'gamma-contacts',
        'gamma-rooms',
        'gamma-nickname',
      ],
    );
    expect(fixture.controller.profiles, isNot(contains('gamma')));
    expect(
      await fixture.options.getStringList('profileDeletionTombstones'),
      const <String>['gamma'],
    );

    storage.failDeleteKeys.clear();
    final restarted = ProfilesController(
      storage: storage,
      options: fixture.options,
      roomHasher: _roomHash,
    );
    await restarted.init(const <String>[]);

    expect(
      await fixture.options.getStringList('profileDeletionTombstones'),
      isEmpty,
    );
    expect(await storage.read(key: 'gamma-keypair'), isNull);
  });

  test('startup uses index as tombstone authority', () async {
    final fixture = await _fixture();
    await fixture.options.setStringList(
      'profileDeletionTombstones',
      const <String>['alpha', 'orphan'],
    );
    await fixture.storage.write(
        key: 'orphan-keypair', value: base64Encode(List<int>.filled(32, 9)));
    final restarted = ProfilesController(
      storage: fixture.storage,
      options: fixture.options,
      roomHasher: _roomHash,
    );

    await restarted.init(const <String>[]);

    expect(await fixture.storage.read(key: 'alpha-keypair'), isNotNull);
    expect(await fixture.storage.read(key: 'orphan-keypair'), isNull);
    expect(await fixture.options.getStringList('profileDeletionTombstones'),
        isEmpty);
  });

  test('sole active deletion commits replacement before old profile removal',
      () async {
    RustLib.initMock(api: _RustApi());
    addTearDown(RustLib.dispose);
    final fixture = await _fixture();
    await fixture.controller
        .removeProfile('beta', telepathy: fixture.telepathy);
    await fixture.controller
        .removeProfile('gamma', telepathy: fixture.telepathy);
    late String replacementId;
    fixture.telepathy.onCommit = () {
      replacementId = fixture.controller.activeProfile;
      expect(replacementId, isNot('alpha'));
      expect(fixture.controller.profiles, contains('alpha'));
      expect(fixture.controller.profiles, contains(replacementId));
    };

    await fixture.controller
        .removeProfile('alpha', telepathy: fixture.telepathy);

    expect(fixture.telepathy.commits, 1);
    expect(fixture.controller.activeProfile, replacementId);
    expect(fixture.controller.profiles.keys, <String>[replacementId]);
  });

  test(
    'interactive direct invitation add validates before profile mutation',
    verifyInteractiveInvitationAddIsAtomic,
  );

  test(
    'direct invitation migration preserves contacts and repairs storage',
    verifyDirectInvitationMigration,
  );
}

String _roomHash({required List<String> peers}) => peers.join('|');

Future<
    ({
      ProfilesController controller,
      SharedPreferencesAsync options,
      FlutterSecureStorage storage,
      _Telepathy telepathy
    })> _fixture({
  SharedPreferencesAsync? options,
  FlutterSecureStorage? storage,
}) async {
  final secureStorage = storage ?? const FlutterSecureStorage();
  final prefs = options ?? SharedPreferencesAsync();
  for (final (String id, int byte) in <(String, int)>[
    ('alpha', 1),
    ('beta', 2),
    ('gamma', 3),
  ]) {
    await secureStorage.write(
      key: '$id-keypair',
      value: base64Encode(List<int>.filled(32, byte)),
    );
    await secureStorage.write(key: '$id-peerId', value: '$id-peer');
    await secureStorage.write(key: '$id-nickname', value: id);
  }
  await prefs
      .setStringList('profilesV2', const <String>['alpha', 'beta', 'gamma']);
  await prefs.setString('activeProfile', 'alpha');
  final controller = ProfilesController(
    storage: secureStorage,
    options: prefs,
    roomHasher: _roomHash,
  );
  await controller.init(const <String>[]);
  return (
    controller: controller,
    options: prefs,
    storage: secureStorage,
    telepathy: _Telepathy(),
  );
}

class _Prepared implements PreparedIdentitySwitch {
  _Prepared(this.onDispose, this.onCommit);
  final void Function() onDispose;
  final Future<void> Function() onCommit;

  @override
  Future<void> commit() => onCommit();

  @override
  void dispose() => onDispose();

  @override
  bool get isDisposed => false;
}

class _RustApi implements RustLibApi {
  @override
  Contact crateTypesContactNew({
    required String nickname,
    required String peerId,
  }) =>
      _PersistedContact(
        id: 'contact-$peerId',
        nickname: nickname,
        peerId: peerId,
        outputVolume: 0.0,
        isDirect: false,
        directInvitation: null,
      );

  @override
  Contact crateTypesContactFromParts({
    required String id,
    required String nickname,
    required String peerId,
    required double outputVolume,
    required bool isDirect,
    String? directInvitation,
  }) {
    // The native bridge is the external boundary for this controller test;
    // Rust unit tests cover its real validation and canonicalization behavior.
    final String? canonicalInvitation = switch (directInvitation) {
      _legacyAddresses => _canonicalInvitation(peerId),
      final String value when value == _canonicalInvitation(peerId) => value,
      _ => null,
    };
    return _PersistedContact(
      id: id,
      nickname: nickname,
      peerId: peerId,
      outputVolume: outputVolume,
      isDirect: isDirect && canonicalInvitation != null,
      directInvitation: canonicalInvitation,
    );
  }

  @override
  (String, Uint8List) crateFlutterUtilsGenerateKeys() =>
      ('replacement-peer', Uint8List.fromList(List<int>.filled(32, 4)));

  @override
  Object? noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

const String _legacyAddresses = '[{"Ip":"203.0.113.42:40142"}]';

String _canonicalInvitation(String peerId) {
  final String payload = jsonEncode(<String, dynamic>{
    'version': 1,
    'peer_id': peerId,
    'addresses': <Map<String, String>>[
      <String, String>{'Ip': '203.0.113.42:40142'},
    ],
  });
  return 'tp1:${base64Url.encode(utf8.encode(payload)).replaceAll('=', '')}';
}

class _PersistedContact implements Contact {
  _PersistedContact({
    required String id,
    required String nickname,
    required String peerId,
    required double outputVolume,
    required bool isDirect,
    required String? directInvitation,
  })  : _id = id,
        _nickname = nickname,
        _peerId = peerId,
        _outputVolume = outputVolume,
        _isDirect = isDirect,
        _directInvitation = directInvitation;

  final String _id;
  String _nickname;
  final String _peerId;
  double _outputVolume;
  bool _isDirect;
  String? _directInvitation;

  @override
  String? directInvitation() => _directInvitation;

  @override
  void dispose() {}

  @override
  PublicKey getPeerId() => _TestPublicKey();

  @override
  String id() => _id;

  @override
  bool idEq({required List<int> id}) => false;

  @override
  bool get isDisposed => false;

  @override
  bool isDirect() => _isDirect;

  @override
  String nickname() => _nickname;

  @override
  double outputVolume() => _outputVolume;

  @override
  String peerId() => _peerId;

  @override
  Contact pubClone() => this;

  @override
  void setDirect({required bool isDirect}) {
    _isDirect = isDirect && _directInvitation != null;
  }

  @override
  void setDirectInvitation({String? invitation}) {
    if (invitation == null) {
      _isDirect = false;
      _directInvitation = null;
      return;
    }
    if (invitation != _canonicalInvitation(_peerId)) {
      throw const DartError(message: 'invalid direct invitation');
    }
    _directInvitation = invitation;
  }

  @override
  void setNickname({required String nickname}) {
    _nickname = nickname;
  }

  @override
  void setOutputVolume({required double decibel}) {
    _outputVolume = decibel;
  }
}

class _TestPublicKey implements PublicKey {
  @override
  void dispose() {}

  @override
  bool get isDisposed => false;
}

class _Telepathy implements Telepathy {
  int commits = 0;
  int disposals = 0;
  Object? prepareError;
  Object? commitError;
  void Function()? onCommit;
  final Completer<void> prepareEntered = Completer<void>();
  final Completer<void> commitEntered = Completer<void>();
  Completer<void>? _prepareRelease;
  Completer<void>? _commitRelease;

  void pausePrepare() => _prepareRelease = Completer<void>();
  void releasePrepare() => _prepareRelease!.complete();
  void pauseCommit() => _commitRelease = Completer<void>();
  void releaseCommit() => _commitRelease!.complete();

  @override
  Future<PreparedIdentitySwitch> prepareIdentitySwitch(
      {required List<int> targetKey,
      required List<Contact> targetContacts}) async {
    if (!prepareEntered.isCompleted) prepareEntered.complete();
    await _prepareRelease?.future;
    final error = prepareError;
    if (error != null) throw error;
    return _Prepared(() => disposals += 1, _commit);
  }

  Future<void> _commit() async {
    commits += 1;
    if (!commitEntered.isCompleted) commitEntered.complete();
    await _commitRelease?.future;
    final error = commitError;
    if (error != null) throw error;
    onCommit?.call();
  }

  @override
  Object? noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);

  @override
  void dispose() {}

  @override
  bool get isDisposed => false;
}

class _ThrowingPreferences extends SharedPreferencesAsync {
  final _ThrowingPreferencesFailures failures = _ThrowingPreferencesFailures();

  @override
  Future<void> setString(String key, String value) {
    if (failures.failActiveWrite && key == 'activeProfile') {
      throw StateError('active profile write failed');
    }
    return super.setString(key, value);
  }

  @override
  Future<void> setStringList(String key, List<String> value) {
    if (failures.failIndexWrite && key == 'profilesV2') {
      failures.failIndexWrite = false;
      throw StateError('profile index write failed');
    }
    return super.setStringList(key, value);
  }
}

class _ThrowingPreferencesFailures {
  bool failActiveWrite = false;
  bool failIndexWrite = false;
}

class _ThrowingStorage extends FlutterSecureStorage {
  bool failDeletes = false;
  int deleteCalls = 0;
  final Set<String> failDeleteKeys = <String>{};
  final List<String> deletedKeys = <String>[];

  @override
  Future<void> delete({
    required String key,
    AppleOptions? iOptions,
    AndroidOptions? aOptions,
    LinuxOptions? lOptions,
    WebOptions? webOptions,
    AppleOptions? mOptions,
    WindowsOptions? wOptions,
  }) {
    deleteCalls += 1;
    deletedKeys.add(key);
    if (failDeletes || failDeleteKeys.contains(key)) {
      throw StateError('secure storage delete failed');
    }
    return super.delete(
      key: key,
      iOptions: iOptions,
      aOptions: aOptions,
      lOptions: lOptions,
      webOptions: webOptions,
      mOptions: mOptions,
      wOptions: wOptions,
    );
  }
}
