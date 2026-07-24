import 'dart:async';
import 'dart:convert';

import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:shared_preferences_platform_interface/in_memory_shared_preferences_async.dart';
import 'package:shared_preferences_platform_interface/shared_preferences_async_platform_interface.dart';
import 'package:telepathy/controllers/profiles_controller.dart';
import 'package:telepathy/core/rust/flutter.dart';
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
    expect(await fixture.options.getString('activeProfile'), 'alpha');
    expect(fixture.telepathy.commits, 0);
  });

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
    if (failDeletes) {
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
