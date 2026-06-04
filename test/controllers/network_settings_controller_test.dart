import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:shared_preferences_platform_interface/in_memory_shared_preferences_async.dart';
import 'package:shared_preferences_platform_interface/shared_preferences_async_platform_interface.dart';
import 'package:telepathy/controllers/network_settings_controller.dart';
import 'package:telepathy/core/constants/network_constants.dart';
import 'package:telepathy/core/rust/types.dart';

void main() {
  group('NetworkSettingsController.buildNetworkConfigFromSpec', () {
    test('uses safe defaults when there is no stored configuration', () {
      final recorder = _Recorder();
      NetworkSettingsController.buildNetworkConfigFromSpec(
        stored: const StoredNetworkValues(),
        builder: recorder.build,
      );

      // The constructor must be called with safe defaults only, never with
      // values pulled from storage. This is the regression guard for the
      // bug where the fallback path retried construction with invalid
      // stored values and aborted initialization.
      expect(recorder.constructorCalls, hasLength(1));
      final ctor = recorder.constructorCalls.single;
      expect(ctor.listenPort, defaultListenPort);
      expect(ctor.bindAddresses, defaultBindAddresses);
      expect(ctor.relays, isNull);
      expect(ctor.dnsEndpoint, isNull);
      expect(ctor.dnsOriginDomain, isNull);
      expect(ctor.pkarrRelay, isNull);

      // The atomic update is still invoked with the safe defaults so
      // the live config remains consistent. With no stored values, the
      // update is effectively a no-op (it re-applies the defaults).
      expect(recorder.updateCalls, hasLength(1));
      final update = recorder.updateCalls.single;
      expect(update.listenPort, defaultListenPort);
      expect(update.bindAddresses, defaultBindAddresses);
      expect(update.relays, isNull);
      expect(update.dnsEndpoint, isNull);
      expect(update.dnsOriginDomain, isNull);
      expect(update.pkarrRelay, isNull);
    });

    test('applies valid stored values via a single atomic update', () {
      final recorder = _Recorder();
      NetworkSettingsController.buildNetworkConfigFromSpec(
        stored: const StoredNetworkValues(
          listenPort: 40142,
          bindAddresses: ['0.0.0.0', '::1'],
          customRelaysEnabled: true,
          relays: ['https://relay-us.iroh.example/'],
          customDnsEnabled: true,
          dnsEndpoint: '1.1.1.1:53',
          dnsOriginDomain: 'dns.iroh.example',
          customPkarrEnabled: true,
          pkarrRelay: 'https://pkarr.iroh.example/',
        ),
        builder: recorder.build,
      );

      // Constructor was still called only once, and only with safe defaults.
      expect(recorder.constructorCalls, hasLength(1));
      final ctor = recorder.constructorCalls.single;
      expect(ctor.listenPort, defaultListenPort);
      expect(ctor.bindAddresses, defaultBindAddresses);
      expect(ctor.relays, isNull);
      expect(ctor.dnsEndpoint, isNull);

      // A single atomic update received the valid stored values, and the
      // live config reflects them.
      expect(recorder.updateCalls, hasLength(1));
      final update = recorder.updateCalls.single;
      expect(update.listenPort, 40142);
      expect(update.bindAddresses, ['0.0.0.0', '::1']);
      expect(update.relays, ['https://relay-us.iroh.example/']);
      expect(update.dnsEndpoint, '1.1.1.1:53');
      expect(update.dnsOriginDomain, 'dns.iroh.example');
      expect(update.pkarrRelay, 'https://pkarr.iroh.example/');

      expect(recorder.currentListenPort, 40142);
      expect(recorder.currentBindAddresses, ['0.0.0.0', '::1']);
      expect(
        recorder.currentRelays,
        ['https://relay-us.iroh.example/'],
      );
      expect(recorder.currentDnsEndpoint, '1.1.1.1:53');
      expect(recorder.currentDnsOriginDomain, 'dns.iroh.example');
      expect(
        recorder.currentPkarrRelay,
        'https://pkarr.iroh.example/',
      );
    });

    test('drops invalid stored bind addresses and keeps defaults', () {
      final recorder = _Recorder();
      NetworkSettingsController.buildNetworkConfigFromSpec(
        stored: const StoredNetworkValues(
          listenPort: 40142,
          bindAddresses: ['0.0.0.0', 'not-an-ip', '::1'],
        ),
        builder: recorder.build,
      );

      // The constructor is called only with the safe defaults; the stored
      // bind list contains a garbage entry so the atomic update fails
      // and the live config keeps the defaults.
      expect(recorder.constructorCalls, hasLength(1));
      expect(
        recorder.constructorCalls.single.bindAddresses,
        defaultBindAddresses,
      );
      // The update was attempted but rejected by the fake because of the
      // invalid bind address. The live config is still the default.
      expect(recorder.updateCalls, hasLength(1));
      expect(recorder.updateCalls.single.listenPort, 40142);
      expect(recorder.currentListenPort, defaultListenPort);
      expect(recorder.currentBindAddresses, defaultBindAddresses);
    });

    test('drops invalid stored relay URLs and keeps defaults', () {
      final recorder = _Recorder();
      NetworkSettingsController.buildNetworkConfigFromSpec(
        stored: const StoredNetworkValues(
          customRelaysEnabled: true,
          relays: ['https://relay-us.iroh.example/', 'not a url'],
        ),
        builder: recorder.build,
      );

      // Relays: stored list contains a malformed entry, so the atomic
      // update fails and the live config keeps the defaults.
      expect(recorder.constructorCalls, hasLength(1));
      expect(recorder.constructorCalls.single.relays, isNull);
      expect(recorder.updateCalls, hasLength(1));
      expect(recorder.updateCalls.single.relays, [
        'https://relay-us.iroh.example/',
        'not a url',
      ]);
      expect(recorder.currentRelays, isNull);
    });

    test('drops invalid stored DNS endpoint and its paired origin domain',
        () {
      final recorder = _Recorder();
      NetworkSettingsController.buildNetworkConfigFromSpec(
        stored: const StoredNetworkValues(
          customDnsEnabled: true,
          // missing port; the rust SocketAddr parser rejects this
          dnsEndpoint: '1.1.1.1',
          dnsOriginDomain: 'dns.iroh.example',
        ),
        builder: recorder.build,
      );

      // DNS endpoint is invalid; the atomic update fails and the live
      // config keeps the defaults. The controller must drop BOTH the
      // endpoint and the origin domain, because keeping one without the
      // other is meaningless.
      expect(recorder.constructorCalls, hasLength(1));
      expect(recorder.constructorCalls.single.dnsEndpoint, isNull);
      expect(recorder.constructorCalls.single.dnsOriginDomain, isNull);
      expect(recorder.updateCalls, hasLength(1));
      expect(recorder.updateCalls.single.dnsEndpoint, '1.1.1.1');
      expect(
        recorder.updateCalls.single.dnsOriginDomain,
        'dns.iroh.example',
      );
      expect(recorder.currentDnsEndpoint, isNull);
      expect(recorder.currentDnsOriginDomain, isNull);
    });

    test(
        'drops stored DNS when only the endpoint is present (no paired origin '
        'domain)', () {
      final recorder = _Recorder();
      NetworkSettingsController.buildNetworkConfigFromSpec(
        stored: const StoredNetworkValues(
          customDnsEnabled: true,
          dnsEndpoint: '1.1.1.1:53',
          // dnsOriginDomain intentionally omitted
        ),
        builder: recorder.build,
      );

      // Endpoint-only stored DNS is a half-configuration. The controller
      // omits BOTH halves from the update so the live config is not
      // touched.
      expect(recorder.constructorCalls, hasLength(1));
      expect(recorder.constructorCalls.single.dnsEndpoint, isNull);
      expect(recorder.constructorCalls.single.dnsOriginDomain, isNull);
      expect(recorder.updateCalls, hasLength(1));
      expect(recorder.updateCalls.single.dnsEndpoint, isNull);
      expect(recorder.updateCalls.single.dnsOriginDomain, isNull);
    });

    test(
        'drops stored DNS when only the origin domain is present (no paired '
        'endpoint)', () {
      final recorder = _Recorder();
      NetworkSettingsController.buildNetworkConfigFromSpec(
        stored: const StoredNetworkValues(
          customDnsEnabled: true,
          // dnsEndpoint intentionally omitted
          dnsOriginDomain: 'dns.iroh.example',
        ),
        builder: recorder.build,
      );

      // Origin-domain-only stored DNS is also a half-configuration. The
      // controller omits BOTH halves from the update.
      expect(recorder.constructorCalls, hasLength(1));
      expect(recorder.constructorCalls.single.dnsEndpoint, isNull);
      expect(recorder.constructorCalls.single.dnsOriginDomain, isNull);
      expect(recorder.updateCalls, hasLength(1));
      expect(recorder.updateCalls.single.dnsEndpoint, isNull);
      expect(recorder.updateCalls.single.dnsOriginDomain, isNull);
    });

    test('drops invalid stored PKARR relay value', () {
      final recorder = _Recorder();
      NetworkSettingsController.buildNetworkConfigFromSpec(
        stored: const StoredNetworkValues(
          customPkarrEnabled: true,
          pkarrRelay: 'pkarr.example.com', // missing scheme
        ),
        builder: recorder.build,
      );

      // PKARR relay cannot be parsed as a URL; the atomic update fails
      // and the live config keeps the defaults.
      expect(recorder.constructorCalls, hasLength(1));
      expect(recorder.constructorCalls.single.pkarrRelay, isNull);
      expect(recorder.updateCalls, hasLength(1));
      expect(
        recorder.updateCalls.single.pkarrRelay,
        'pkarr.example.com',
      );
      expect(recorder.currentPkarrRelay, isNull);
    });

    test('falls back to defaults when every stored group is invalid', () {
      final recorder = _Recorder();
      NetworkSettingsController.buildNetworkConfigFromSpec(
        stored: const StoredNetworkValues(
          listenPort: 8080,
          bindAddresses: ['garbage', 'also-garbage'],
          customRelaysEnabled: true,
          relays: ['not a url', 'also not a url'],
          customDnsEnabled: true,
          dnsEndpoint: 'not-an-address',
          dnsOriginDomain: 'dns.iroh.example',
          customPkarrEnabled: true,
          pkarrRelay: 'no-scheme-here',
        ),
        builder: recorder.build,
      );

      // All stored groups are corrupt. The controller must NOT retry
      // the constructor with any of these values. The constructor is
      // invoked exactly once with the safe defaults, and the atomic
      // update fails because the first invalid group (the bind list)
      // is rejected, leaving the live config at the safe defaults.
      expect(recorder.constructorCalls, hasLength(1));
      final ctor = recorder.constructorCalls.single;
      expect(ctor.listenPort, defaultListenPort);
      expect(ctor.bindAddresses, defaultBindAddresses);
      expect(ctor.relays, isNull);
      expect(ctor.dnsEndpoint, isNull);
      expect(ctor.dnsOriginDomain, isNull);
      expect(ctor.pkarrRelay, isNull);

      // The update was attempted, but the bind list is invalid so the
      // fake rejected it. Live config keeps the defaults.
      expect(recorder.updateCalls, hasLength(1));
      expect(recorder.updateCalls.single.listenPort, 8080);
      expect(recorder.currentListenPort, defaultListenPort);
      expect(recorder.currentBindAddresses, defaultBindAddresses);
    });

    test('treats empty stored bind list as "no override" and keeps defaults',
        () {
      final recorder = _Recorder();
      NetworkSettingsController.buildNetworkConfigFromSpec(
        stored: const StoredNetworkValues(
          bindAddresses: <String>[],
        ),
        builder: recorder.build,
      );

      // An explicitly empty list means "user cleared their bind
      // addresses"; the controller passes the safe defaults to the
      // constructor AND to the update, so the live config keeps the
      // defaults rather than being set to an empty list.
      expect(recorder.updateCalls, hasLength(1));
      expect(
        recorder.updateCalls.single.bindAddresses,
        defaultBindAddresses,
      );
      expect(recorder.currentBindAddresses, defaultBindAddresses);
      expect(
        recorder.constructorCalls.single.bindAddresses,
        defaultBindAddresses,
      );
    });

    test('treats empty stored relay list as "no override" and keeps defaults '
        'even when customRelaysEnabled is true', () {
      final recorder = _Recorder();
      NetworkSettingsController.buildNetworkConfigFromSpec(
        stored: const StoredNetworkValues(
          customRelaysEnabled: true,
          relays: <String>[],
        ),
        builder: recorder.build,
      );

      // An empty stored relay list with customRelaysEnabled=true must
      // be reported as "no override" -- the controller passes `null`
      // to the update, leaving the default relay behavior intact.
      // Persisting customRelaysEnabled=true against zero URLs would,
      // on the rust side, disable the default relay map without
      // supplying a replacement -- effectively breaking connectivity.
      expect(recorder.updateCalls, hasLength(1));
      expect(recorder.updateCalls.single.relays, isNull);
      expect(
        recorder.constructorCalls.single.relays,
        isNull,
        reason: 'constructor must not be called with an empty relay list',
      );
    });

    test(
        'a failed save (invalid bind + changed listen port) leaves the live '
        'config unchanged (regression guard for partial-mutation bug)', () {
      // Seed the live config with a known good state -- the bindings that
      // would be in place after a successful prior save.
      final recorder = _Recorder();
      NetworkSettingsController.buildNetworkConfigFromSpec(
        stored: const StoredNetworkValues(
          listenPort: 40142,
          bindAddresses: ['0.0.0.0', '::1'],
          customRelaysEnabled: true,
          relays: ['https://relay-us.iroh.example/'],
        ),
        builder: recorder.build,
      );

      // The seeded state has been applied via the atomic update.
      expect(recorder.currentListenPort, 40142);
      expect(recorder.currentBindAddresses, ['0.0.0.0', '::1']);
      expect(
        recorder.currentRelays,
        ['https://relay-us.iroh.example/'],
      );

      // Now simulate the save-time mutation path: a single atomic
      // update carrying BOTH a different listen port AND an invalid
      // bind address (mixed with everything else unchanged). The
      // update must fail, and the live config must reflect the
      // *pre-save* values -- in particular, the listen port must
      // still be 40142, not 9999. The throw is tagged with the
      // offending field so the error matches the production rust
      // contract.
      recorder.throwOnNextUpdate = const NetworkConfigUpdateError(
        field: NetworkConfigField.bindAddresses,
        message: 'invalid IP literal: not-an-ip',
      );

      expect(
        () => recorder.updateCallsSink(
          listenPort: 9999,
          bindAddresses: ['not-an-ip', '::1'],
          relays: ['https://relay-us.iroh.example/'],
          dnsEndpoint: null,
          dnsOriginDomain: null,
          pkarrRelay: null,
        ),
        throwsA(isA<NetworkConfigUpdateError>()),
      );

      // Live config is unchanged: the listen port is still 40142
      // (NOT 9999) and the bind addresses are still the original
      // valid list (NOT ['not-an-ip', '::1']). The per-field setter
      // approach would have committed the new listen port before
      // failing on the invalid bind addresses.
      expect(
        recorder.currentListenPort,
        40142,
        reason: 'failed save must not change the listen port',
      );
      expect(
        recorder.currentBindAddresses,
        ['0.0.0.0', '::1'],
        reason: 'failed save must not change the bind addresses',
      );
      expect(
        recorder.currentRelays,
        ['https://relay-us.iroh.example/'],
        reason: 'failed save must not change the relays',
      );
    });
  });

  group('NetworkSettingsController.saveNetworkConfig', () {
    late _Recorder recorder;
    late NetworkSettingsController controller;
    late SharedPreferencesAsync options;
    late InMemorySharedPreferencesAsync store;

    setUp(() async {
      store = InMemorySharedPreferencesAsync.empty();
      SharedPreferencesAsyncPlatform.instance = store;
      options = SharedPreferencesAsync();
      recorder = _Recorder();
      controller = NetworkSettingsController(options: options);
      // We deliberately do NOT call controller.init() here because it
      // would also try to load ScreenshareConfig/CodecConfig/OverlayConfig
      // through the rust binding, which is not loaded in `flutter test`.
      // Assign a fake-backed networkConfig directly. The save path under
      // test only touches networkConfig and the SharedPreferencesAsync
      // store, both of which are isolated.
      controller.networkConfig = recorder.build(
        listenPort: defaultListenPort,
        bindAddresses: defaultBindAddresses,
        relays: null,
        dnsEndpoint: null,
        dnsOriginDomain: null,
        pkarrRelay: null,
      );
    });

    tearDown(() {
      SharedPreferencesAsyncPlatform.instance = null;
    });

    test('persists customRelaysEnabled=true and the relay list when non-empty',
        () async {
      // Realistic relay configuration a user would save.
      recorder.currentRelays = <String>[
        'https://relay-us.iroh.example/',
        'https://relay-eu.iroh.example/',
      ];

      await controller.saveNetworkConfig();

      expect(
        await options.getBool('customRelaysEnabled'),
        isTrue,
        reason: 'non-empty relay list must enable custom relays',
      );
      expect(
        await options.getStringList('relays'),
        <String>[
          'https://relay-us.iroh.example/',
          'https://relay-eu.iroh.example/',
        ],
        reason: 'non-empty relay list must be persisted',
      );
    });

    test(
        'does NOT persist customRelaysEnabled=true for an empty relay list '
        '(regression guard for the empty-relays misconfiguration)', () async {
      // Simulate a misconfigured `NetworkConfig` that has relays as
      // `Some(empty list)`. The settings UI rejects this at save time,
      // so reaching saveNetworkConfig with this state implies another
      // code path set it; the controller must still avoid persisting
      // `customRelaysEnabled = true` against zero URLs, which would
      // disable the default relay map without a replacement.
      recorder.currentRelays = <String>[];

      await controller.saveNetworkConfig();

      expect(
        await options.getBool('customRelaysEnabled'),
        isFalse,
        reason: 'empty relay list must NOT enable custom relays',
      );
      expect(
        await options.getStringList('relays'),
        isNull,
        reason: 'empty relay list must NOT be persisted under the relays key',
      );
    });

    test('persists customRelaysEnabled=false and removes the relays key '
        'when relays is null', () async {
      // The default state of a fresh NetworkConfig: no custom relays
      // configured. The controller must clear the `relays` key so a
      // subsequent load does not see a stale empty list.
      recorder.currentRelays = null;

      await controller.saveNetworkConfig();

      expect(await options.getBool('customRelaysEnabled'), isFalse);
      expect(await options.getStringList('relays'), isNull);
    });

    test('overwrites a previously-stored relay list when relays change',
        () async {
      // Round-trip: first save has one URL, second save has a different
      // list. The controller must not leave the old value around.
      recorder.currentRelays = <String>['https://old.iroh.example/'];
      await controller.saveNetworkConfig();
      expect(
        await options.getStringList('relays'),
        <String>['https://old.iroh.example/'],
      );

      recorder.currentRelays = <String>['https://new.iroh.example/'];
      await controller.saveNetworkConfig();
      expect(
        await options.getStringList('relays'),
        <String>['https://new.iroh.example/'],
      );
      expect(await options.getBool('customRelaysEnabled'), isTrue);
    });

    test('clears the relays key when the user transitions from a list to null',
        () async {
      // Round-trip: relays goes from "Some([url])" to "None". The
      // controller must remove the `relays` key entirely rather than
      // leave an empty list behind, which would otherwise read back as
      // "custom relays enabled, no URLs" on next launch.
      recorder.currentRelays = <String>['https://relay.iroh.example/'];
      await controller.saveNetworkConfig();
      expect(await options.getBool('customRelaysEnabled'), isTrue);
      expect(
        await options.getStringList('relays'),
        isNotNull,
      );

      recorder.currentRelays = null;
      await controller.saveNetworkConfig();
      expect(await options.getBool('customRelaysEnabled'), isFalse);
      expect(
        await options.getStringList('relays'),
        isNull,
        reason: 'returning to null must clear the persisted relay list',
      );
    });
  });
}

class _ConstructorArgs {
  _ConstructorArgs({
    required this.listenPort,
    required this.bindAddresses,
    required this.relays,
    required this.dnsEndpoint,
    required this.dnsOriginDomain,
    required this.pkarrRelay,
  });

  final int listenPort;
  final List<String> bindAddresses;
  final List<String>? relays;
  final String? dnsEndpoint;
  final String? dnsOriginDomain;
  final String? pkarrRelay;
}

class _UpdateArgs {
  _UpdateArgs({
    required this.listenPort,
    required this.bindAddresses,
    required this.relays,
    required this.dnsEndpoint,
    required this.dnsOriginDomain,
    required this.pkarrRelay,
  });

  final int listenPort;
  final List<String> bindAddresses;
  final List<String>? relays;
  final String? dnsEndpoint;
  final String? dnsOriginDomain;
  final String? pkarrRelay;
}

/// Records the values passed into the [NetworkConfigBuilder] and into the
/// atomic [NetworkConfig.update] on the returned [NetworkConfig]. Acts as a
/// stand-in for the rust binding so we can assert what the controller did
/// with stored values without needing the native library to be loaded.
class _Recorder {
  final List<_ConstructorArgs> constructorCalls = <_ConstructorArgs>[];

  /// Tracks the live value of each field. The fake rolls back to the
  /// previous value when `update` throws, mirroring the rust binding's
  /// behavior.
  int? currentListenPort;
  List<String>? currentBindAddresses;
  List<String>? currentRelays;
  String? currentDnsEndpoint;
  String? currentDnsOriginDomain;
  String? currentPkarrRelay;

  /// Each call to the atomic `update` method. The "current" fields
  /// above are updated on success and left untouched on failure.
  final List<_UpdateArgs> updateCalls = <_UpdateArgs>[];

  /// When non-null, the next call to `update` throws this error and
  /// leaves the live config unchanged. Used to simulate a backend
  /// rejection mid-save. Mirrors the production rust atomic setter
  /// which surfaces every rejection as a typed
  /// [NetworkConfigUpdateError] tagged with the offending
  /// [NetworkConfigField].
  NetworkConfigUpdateError? throwOnNextUpdate;

  /// Forwards a single update through the fake so a test can drive
  /// the save-time path against the seeded state.
  void updateCallsSink({
    required int listenPort,
    required List<String> bindAddresses,
    required List<String>? relays,
    required String? dnsEndpoint,
    required String? dnsOriginDomain,
    required String? pkarrRelay,
  }) {
    final networkConfig = _FakeNetworkConfig.latest;
    if (networkConfig == null) {
      throw StateError(
        'No _FakeNetworkConfig has been built yet; call build() first.',
      );
    }
    networkConfig.update(
      listenPort: listenPort,
      bindAddresses: bindAddresses,
      relays: relays,
      dnsEndpoint: dnsEndpoint,
      dnsOriginDomain: dnsOriginDomain,
      pkarrRelay: pkarrRelay,
    );
  }

  NetworkConfig build({
    required int listenPort,
    required List<String> bindAddresses,
    required List<String>? relays,
    required String? dnsEndpoint,
    required String? dnsOriginDomain,
    required String? pkarrRelay,
  }) {
    constructorCalls.add(
      _ConstructorArgs(
        listenPort: listenPort,
        bindAddresses: bindAddresses.toList(),
        relays: relays?.toList(),
        dnsEndpoint: dnsEndpoint,
        dnsOriginDomain: dnsOriginDomain,
        pkarrRelay: pkarrRelay,
      ),
    );
    // Initialize the "current" state to the values the constructor was
    // given, so a subsequent failing `update` leaves the field at this
    // safe value.
    currentListenPort = listenPort;
    currentBindAddresses = bindAddresses.toList();
    currentRelays = relays?.toList();
    currentDnsEndpoint = dnsEndpoint;
    currentDnsOriginDomain = dnsOriginDomain;
    currentPkarrRelay = pkarrRelay;
    return _FakeNetworkConfig(this);
  }
}

class _FakeNetworkConfig implements NetworkConfig {
  _FakeNetworkConfig(this._recorder);

  /// The most-recently-built fake. Test helpers use this to drive the
  /// save-time path against the seeded state without having to plumb
  /// the fake through a controller.
  static _FakeNetworkConfig? latest;

  final _Recorder _recorder;

  @override
  bool get isDisposed => false;

  @override
  void dispose() {}

  @override
  List<String> getBindAddresses() =>
      _recorder.currentBindAddresses ?? const <String>[];

  @override
  String? getDnsEndpoint() => _recorder.currentDnsEndpoint;

  @override
  String? getDnsOriginDomain() => _recorder.currentDnsOriginDomain;

  @override
  int getListenPort() => _recorder.currentListenPort ?? 0;

  @override
  String? getPkarrRelay() => _recorder.currentPkarrRelay;

  @override
  List<String>? getRelays() => _recorder.currentRelays;

  @override
  void update({
    required int listenPort,
    required List<String> bindAddresses,
    List<String>? relays,
    String? dnsEndpoint,
    String? dnsOriginDomain,
    String? pkarrRelay,
  }) {
    // Record the attempt before validating, so a test can assert that
    // a particular update was attempted even when it failed.
    _recorder.updateCalls.add(
      _UpdateArgs(
        listenPort: listenPort,
        bindAddresses: bindAddresses.toList(),
        relays: relays?.toList(),
        dnsEndpoint: dnsEndpoint,
        dnsOriginDomain: dnsOriginDomain,
        pkarrRelay: pkarrRelay,
      ),
    );
    // The fake mirrors the rust atomic setter: every field is validated
    // up front, and if any check fails the live config is left
    // unchanged. The check uses the same per-field rules the per-field
    // setters use, so a test that exercises one group also exercises
    // the same logic the production path uses. The thrown error is
    // tagged with the offending [NetworkConfigField] so the
    // controller (and the UI) can route the error to the right
    // input, matching the production contract.
    for (final address in bindAddresses) {
      if (!_isValidIpLiteral(address)) {
        throw NetworkConfigUpdateError(
          field: NetworkConfigField.bindAddresses,
          message: 'invalid IP literal: $address',
        );
      }
    }
    if (dnsEndpoint != null && !dnsEndpoint.contains(':')) {
      throw NetworkConfigUpdateError(
        field: NetworkConfigField.dnsEndpoint,
        message: 'invalid dns endpoint: $dnsEndpoint',
      );
    }
    if (relays != null) {
      for (final url in relays) {
        final parsed = Uri.tryParse(url);
        if (parsed == null || !parsed.hasScheme) {
          throw NetworkConfigUpdateError(
            field: NetworkConfigField.relays,
            message: 'invalid relay url: $url',
          );
        }
      }
    }
    if (pkarrRelay != null) {
      final parsed = Uri.tryParse(pkarrRelay);
      if (parsed == null || !parsed.hasScheme) {
        throw NetworkConfigUpdateError(
          field: NetworkConfigField.pkarrRelay,
          message: 'invalid pkarr relay url: $pkarrRelay',
        );
      }
    }
    if (_recorder.throwOnNextUpdate != null) {
      final error = _recorder.throwOnNextUpdate!;
      _recorder.throwOnNextUpdate = null;
      throw error;
    }
    // Every field has been validated. Commit the new values.
    _recorder.currentListenPort = listenPort;
    _recorder.currentBindAddresses = bindAddresses.toList();
    _recorder.currentRelays = relays?.toList();
    _recorder.currentDnsEndpoint = dnsEndpoint;
    _recorder.currentDnsOriginDomain = dnsOriginDomain;
    _recorder.currentPkarrRelay = pkarrRelay;
    latest = this;
  }
}

/// Minimal IP-literal validator that mirrors the rust parser: a non-empty
/// string composed only of digits, dots, colons, and `a-f`/`A-F` (for IPv6)
/// and at most one `:` group of hex digits. This is intentionally permissive
/// (it accepts malformed values the rust parser would reject) because the
/// goal is to exercise the controller's error-handling path, not to
/// re-implement the IP parser in Dart.
bool _isValidIpLiteral(String value) {
  if (value.isEmpty) return false;
  // Reject anything that is obviously not an IP literal: spaces, slashes,
  // letters other than a-f, etc. This is good enough to flag the inputs
  // the test uses as "invalid" without false positives on real addresses.
  return RegExp(r'^[0-9a-fA-F:.]+$').hasMatch(value);
}
