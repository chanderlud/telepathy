import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:shared_preferences_platform_interface/in_memory_shared_preferences_async.dart';
import 'package:shared_preferences_platform_interface/shared_preferences_async_platform_interface.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/core/rust/flutter.dart';
import 'package:telepathy/core/rust/lib.dart';
import 'package:telepathy/core/rust/types.dart';
import 'package:telepathy/screens/settings/sections/networking.dart';
import 'package:telepathy/widgets/common/index.dart';

void main() {
  setUp(() {
    SharedPreferencesAsyncPlatform.instance =
        InMemorySharedPreferencesAsync.empty();
  });

  tearDown(() {
    SharedPreferencesAsyncPlatform.instance = null;
  });

  testWidgets(
      'a save with an invalid bind address shows the error and re-enables the '
      'Save button for retry', (WidgetTester tester) async {
    // Start from a known good network config. The user is going to
    // change the listen port AND introduce an invalid bind address.
    // The save must fail (so we should see the error) and the Save
    // button must be re-enabled so the user can retry.
    final recorder = _NetworkConfigRecorder(
      listenPort: 40142,
      bindAddresses: const ['0.0.0.0', '::'],
    );
    final controller = _FakeNetworkSettingsController(recorder);
    final stateController = StateController();
    final telepathy = _FakeTelepathy();

    await tester.pumpNetworkSettings(
      controller: controller,
      stateController: stateController,
      telepathy: telepathy,
    );

    // The form must initially render the listen port and bind
    // addresses the recorder seeded the live config with -- the
    // fake reads the seeded values back through
    // [NetworkConfig.getListenPort] / [NetworkConfig.getBindAddresses]
    // on first build, and the widget mirrors them into the visible
    // inputs. If the fake ever regresses and starts handing back
    // zeros / empty lists, this assertion catches it before the
    // save-path logic is exercised.
    expect(
      find.widgetWithText(TextField, '40142'),
      findsOneWidget,
      reason: 'listen port input must render the seeded value',
    );
    expect(
      find.widgetWithText(TextField, '0.0.0.0, ::'),
      findsOneWidget,
      reason: 'bind addresses input must render the seeded values',
    );

    // Modify the listen port and enter an invalid bind address.
    await tester.enterText(
      find.widgetWithText(TextField, 'Listen Port'),
      '40199',
    );
    await tester.enterText(
      find.widgetWithText(TextField, 'Bind Addresses'),
      'not-an-ip, ::1',
    );
    await tester.pump();

    // The dirty check should mark the form as having unsaved changes,
    // which surfaces the Save button.
    final saveButton = find.widgetWithText(ElevatedButton, 'Save Changes');
    expect(saveButton, findsOneWidget);

    // No error visible yet -- we haven't tried to save.
    expect(
      find.text('Enter at least one bind address'),
      findsNothing,
    );

    // Tap Save. The client-side validation rejects the bind list
    // (an "invalid IP literal" is not in the controller's
    // per-field-validator set, so it falls through to the atomic
    // update which the fake rejects with the same error message the
    // real rust binding would produce for an invalid bind address).
    await tester.tap(saveButton);
    // Pump the future returned by saveChanges() and let the
    // setState-driven rebuild settle.
    await tester.pump();
    await tester.pump();

    // The error from the backend must be visible to the user.
    expect(
      find.text('invalid IP literal: not-an-ip'),
      findsOneWidget,
      reason: 'failed save must surface the backend error on the form',
    );

    // The form is still dirty (the save did not stick), so the Save
    // button is still rendered -- and crucially, it must be ENABLED
    // so the user can retry. The previous bug left `_isSaving = true`
    // on this branch, which disabled the button forever.
    expect(saveButton, findsOneWidget);
    // The custom `Button` widget wraps an `ElevatedButton` and uses
    // its own `disabled` flag to gate the click. Inspect that flag
    // directly because the `ElevatedButton.onPressed` callback is
    // always non-null -- it short-circuits internally when disabled.
    final Button saveButtonWidget = tester.widget<Button>(
      find.ancestor(of: saveButton, matching: find.byType(Button)),
    );
    expect(
      saveButtonWidget.disabled,
      isFalse,
      reason:
          'Save button must be re-enabled after a failed save so the user can retry',
    );
  });

  testWidgets(
      'a typed DNS endpoint update error renders under the DNS Endpoint '
      'input', (WidgetTester tester) async {
    // The fake's own DNS validation can only reject an endpoint that
    // is missing a `:`. The client-side `_validateDnsEndpoint` rejects
    // the same shape, so the in-fake check is unreachable from the
    // form. To exercise the typed-error routing for a non-bind field
    // we drive the form with a valid host:port and inject the typed
    // [NetworkConfigUpdateError] through the recorder's
    // `throwOnNextUpdate` slot. This mirrors what the rust side
    // produces when the [NetworkConfig::update] setter rejects a
    // DNS endpoint that the frontend client-side validator accepted
    // (e.g. a port out of range, or a future tightening on the rust
    // side that the frontend has not yet mirrored).
    final recorder = _NetworkConfigRecorder(
      listenPort: 40142,
      bindAddresses: const ['0.0.0.0', '::'],
    );
    recorder.throwOnNextUpdate = const NetworkConfigUpdateError(
      field: NetworkConfigField.dnsEndpoint,
      message: 'invalid dns endpoint: 127.0.0.1:not-a-port',
    );
    final controller = _FakeNetworkSettingsController(recorder);
    final stateController = StateController();
    final telepathy = _FakeTelepathy();

    await tester.pumpNetworkSettings(
      controller: controller,
      stateController: stateController,
      telepathy: telepathy,
    );

    // Enable custom DNS and enter a syntactically valid host:port
    // endpoint, plus the trailing-dot origin domain. The fake
    // accepts the host:port shape (it only checks `contains(':')`),
    // but the recorder is staged to throw a typed
    // [NetworkConfigField.dnsEndpoint] error on the next update.
    // The CustomSwitch for the DNS section is the one that shares a
    // parent Row with the "Use Custom DNS" label. We locate it that
    // way because the layout's right-aligned switches can land just
    // past the hit-test boundary of the parent scroll view, so a
    // blind `find.byType(CustomSwitch).at(1)` tap misses.
    final dnsSwitch = find.descendant(
      of: find.ancestor(
        of: find.text('Use Custom DNS'),
        matching: find.byType(Row),
      ),
      matching: find.byType(CustomSwitch),
    );
    expect(dnsSwitch, findsOneWidget);
    await tester.ensureVisible(dnsSwitch);
    await tester.pumpAndSettle();
    await tester.tap(dnsSwitch, warnIfMissed: false);
    await tester.pumpAndSettle();
    // Sanity check: the DNS fields only render when the custom-DNS
    // toggle is ON. If the tap missed, the fields will not be in
    // the tree and the subsequent `enterText` calls would target
    // nothing.
    expect(find.widgetWithText(TextField, 'DNS Endpoint'), findsOneWidget);
    // The form renders the bind addresses the recorder seeded
    // the live config with, so client-side validation passes and
    // the save path reaches the fake -- which is where the staged
    // DNS endpoint error fires.
    await tester.enterText(
      find.widgetWithText(TextField, 'DNS Endpoint'),
      '127.0.0.1:5353',
    );
    await tester.enterText(
      find.widgetWithText(TextField, 'DNS Origin Domain'),
      '_iroh.example.com.',
    );
    await tester.pump();

    final saveButton = find.widgetWithText(ElevatedButton, 'Save Changes');
    expect(saveButton, findsOneWidget);
    await tester.ensureVisible(saveButton);
    await tester.pumpAndSettle();

    await tester.tap(saveButton, warnIfMissed: false);
    await tester.pump();
    await tester.pump();

    // The typed error must be surfaced on the DNS Endpoint input
    // itself (not the backend banner, not a sibling field). The
    // widget routes the message to the per-field error slot whose
    // [NetworkConfigField] tag matches; every other slot is left
    // null by the surrounding `_clearErrors()` call.
    expect(
      find.text('invalid dns endpoint: 127.0.0.1:not-a-port'),
      findsOneWidget,
      reason:
          'typed NetworkConfigField.dnsEndpoint errors must render under the DNS Endpoint input',
    );

    // The backend-error banner must NOT be triggered for a
    // per-field failure -- this is the discriminator that lets a
    // poison-failure stand out from a normal validation error.
    expect(
      find.textContaining('Backend error:'),
      findsNothing,
      reason:
          'a per-field NetworkConfigField.dnsEndpoint error must not surface in the backend-error banner',
    );

    // The Save button must be re-enabled for retry, exactly as for
    // the bind-addresses failure above.
    final Button saveButtonWidget = tester.widget<Button>(
      find.ancestor(of: saveButton, matching: find.byType(Button)),
    );
    expect(
      saveButtonWidget.disabled,
      isFalse,
      reason:
          'Save button must be re-enabled after a typed DNS endpoint error so the user can retry',
    );
  });

  testWidgets(
      'a typed backendError update error renders in the backend error banner '
      'and is not attributed to a field', (WidgetTester tester) async {
    // The rust atomic `update` collapses every poisoned-lock failure
    // into [NetworkConfigField.backendError]. The frontend must
    // surface that in the dedicated backend-error banner (not on
    // any user-supplied input) so the user understands the rust
    // runtime is in a bad state, rather than blaming whichever
    // input happened to be focused. Drive a clean save path and
    // stage a typed backendError on the recorder to exercise this
    // branch end-to-end.
    final recorder = _NetworkConfigRecorder(
      listenPort: 40142,
      bindAddresses: const ['0.0.0.0', '::'],
    );
    recorder.throwOnNextUpdate = const NetworkConfigUpdateError(
      field: NetworkConfigField.backendError,
      message: 'poisoned lock: session manager',
    );
    final controller = _FakeNetworkSettingsController(recorder);
    final stateController = StateController();
    final telepathy = _FakeTelepathy();

    await tester.pumpNetworkSettings(
      controller: controller,
      stateController: stateController,
      telepathy: telepathy,
    );

    // The form renders the bind addresses the recorder seeded
    // the live config with, so client-side validation passes and
    // the save path reaches the fake -- which is where the staged
    // backend error fires.
    //
    // Make a trivial change to the listen port so the form is
    // dirty and the Save button surfaces. The recorder's
    // `throwOnNextUpdate` will fire on the first call to `update`,
    // regardless of which field changed.
    await tester.enterText(
      find.widgetWithText(TextField, 'Listen Port'),
      '40143',
    );
    await tester.pump();

    final saveButton = find.widgetWithText(ElevatedButton, 'Save Changes');
    expect(saveButton, findsOneWidget);
    await tester.ensureVisible(saveButton);
    await tester.pumpAndSettle();

    await tester.tap(saveButton, warnIfMissed: false);
    await tester.pumpAndSettle();

    // The backend-error banner must show the rust message verbatim,
    // prefixed with the literal "Backend error:" label that the
    // widget renders. This is the only place a poison-failure
    // class of error should be surfaced.
    expect(
      find.text(
        'Backend error: poisoned lock: session manager',
      ),
      findsOneWidget,
      reason:
          'typed NetworkConfigField.backendError errors must render in the dedicated backend-error banner',
    );

    // The error must NOT be attributed to any per-input slot -- in
    // particular it must not leak onto the listen-port or
    // bind-addresses inputs, which is what the previous behaviour
    // did (every backend failure was painted on bind). The
    // message must appear only inside the banner, prefixed with
    // "Backend error:".
    expect(
      find.text('poisoned lock: session manager'),
      findsNothing,
      reason:
          'the bare backend-error message must not appear on its own -- the widget renders it only with the "Backend error:" prefix inside the banner',
    );

    // The Save button must be re-enabled so the user can retry.
    final Button saveButtonWidget = tester.widget<Button>(
      find.ancestor(of: saveButton, matching: find.byType(Button)),
    );
    expect(
      saveButtonWidget.disabled,
      isFalse,
      reason:
          'Save button must be re-enabled after a backend error so the user can retry',
    );
  });
}

extension on WidgetTester {
  // Wider than the 650px breakpoint so the inputs render in two
  // columns; the breakpoint logic is only cosmetic, but the value is
  // used by the build layout to keep the test stable.
  static const BoxConstraints _testConstraints = BoxConstraints(
    maxWidth: 1200,
    maxHeight: 1000,
  );

  Future<void> pumpNetworkSettings({
    required NetworkSettingsController controller,
    required StateController stateController,
    required Telepathy telepathy,
  }) async {
    return pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<NetworkSettingsController>.value(
            value: controller,
          ),
          ChangeNotifierProvider<StateController>.value(
            value: stateController,
          ),
          Provider<Telepathy>.value(value: telepathy),
        ],
        child: const MaterialApp(
          home: Scaffold(
            body: SingleChildScrollView(
              child: NetworkSettings(constraints: _testConstraints),
            ),
          ),
        ),
      ),
    );
  }
}

/// Minimal fake for [NetworkSettingsController] that uses an in-test
/// [NetworkConfig] recorder. We deliberately do not call the real
/// `init()` because that would also try to load
/// ScreenshareConfig/CodecConfig/OverlayConfig through the rust
/// binding, which is not loaded in `flutter test`. The save path under
/// test only touches `networkConfig`, `saveNetworkConfig`, and
/// `Telepathy.restartManager`, all of which are isolated via fakes.
class _FakeNetworkSettingsController extends NetworkSettingsController {
  _FakeNetworkSettingsController(this._recorder) : super(options: _options);

  static final SharedPreferencesAsync _options = SharedPreferencesAsync();
  final _NetworkConfigRecorder _recorder;

  @override
  NetworkConfig get networkConfig => _recorder.buildLatest();
}

class _NetworkConfigRecorder {
  _NetworkConfigRecorder({
    required this.listenPort,
    required this.bindAddresses,
    this.relays,
    this.dnsEndpoint,
    this.dnsOriginDomain,
    this.pkarrRelay,
  });

  final int listenPort;
  final List<String> bindAddresses;
  final List<String>? relays;
  final String? dnsEndpoint;
  final String? dnsOriginDomain;
  final String? pkarrRelay;

  /// Set to a non-null value to make the next `update` call throw.
  /// Mirrors the rust binding's "validate then commit" semantics:
  /// validation errors leave every field unchanged. The recorder
  /// itself only performs the in-fake field validation; tests that
  /// need to exercise paths which cannot be reached from the fake's
  /// own validation (e.g. a DNS endpoint that is shaped correctly
  /// enough to pass the client-side and in-fake checks, or a critical
  /// backend error) inject a typed [NetworkConfigUpdateError] (or
  /// [DartError]) through this slot. Typed as `Object?` so both
  /// `FrbException` flavours can be staged.
  Object? throwOnNextUpdate;

  NetworkConfig? _built;

  NetworkConfig buildLatest() {
    return _built ??= _FakeNetworkConfig(this);
  }

  void reset() {
    _built = null;
    throwOnNextUpdate = null;
  }
}

class _FakeNetworkConfig implements NetworkConfig {
  _FakeNetworkConfig(this._recorder)
      : _currentListenPort = _recorder.listenPort,
        _currentBindAddresses = List<String>.from(_recorder.bindAddresses),
        _currentRelays = _recorder.relays == null
            ? null
            : List<String>.from(_recorder.relays!),
        _currentDnsEndpoint = _recorder.dnsEndpoint,
        _currentDnsOriginDomain = _recorder.dnsOriginDomain,
        _currentPkarrRelay = _recorder.pkarrRelay;

  final _NetworkConfigRecorder _recorder;

  // Live state -- mutated only when `update` succeeds. Initialised
  // from the recorder so that the fake mirrors the seeded
  // configuration back through the [NetworkConfig] getters on
  // first build, just like a real rust-backed config would.
  int _currentListenPort;
  List<String> _currentBindAddresses;
  List<String>? _currentRelays;
  String? _currentDnsEndpoint;
  String? _currentDnsOriginDomain;
  String? _currentPkarrRelay;

  @override
  bool get isDisposed => false;

  @override
  void dispose() {}

  @override
  List<String> getBindAddresses() => _currentBindAddresses;

  @override
  String? getDnsEndpoint() => _currentDnsEndpoint;

  @override
  String? getDnsOriginDomain() => _currentDnsOriginDomain;

  @override
  int getListenPort() => _currentListenPort;

  @override
  String? getPkarrRelay() => _currentPkarrRelay;

  @override
  List<String>? getRelays() => _currentRelays;

  @override
  void update({
    required int listenPort,
    required List<String> bindAddresses,
    List<String>? relays,
    String? dnsEndpoint,
    String? dnsOriginDomain,
    String? pkarrRelay,
  }) {
    // Validate first, mirroring the rust binding's atomic setter. The
    // rust side reports validation failures as a typed
    // [NetworkConfigUpdateError] tagged with the offending
    // [NetworkConfigField]; the fake does the same so the widget's
    // typed-error routing (per-field slot for typed fields, dedicated
    // backend-error banner for [NetworkConfigField.backendError]) is
    // exercised end-to-end.
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
    // All checks passed: commit. The widget reads these back via
    // the getters to refresh the canonical form state.
    _currentListenPort = listenPort;
    _currentBindAddresses = bindAddresses;
    _currentRelays = relays;
    _currentDnsEndpoint = dnsEndpoint;
    _currentDnsOriginDomain = dnsOriginDomain;
    _currentPkarrRelay = pkarrRelay;
  }
}

/// Minimal IP-literal validator that mirrors the rust parser: a non-empty
/// string composed only of digits, dots, colons, and `a-f`/`A-F` (for IPv6).
bool _isValidIpLiteral(String value) {
  if (value.isEmpty) return false;
  return RegExp(r'^[0-9a-fA-F:.]+$').hasMatch(value);
}

class _FakeTelepathy implements Telepathy {
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
  }) {
    throw UnimplementedError();
  }

  @override
  Future<void> endCall() async {}

  @override
  Future<void> joinRoom({required List<String> memberStrings}) async {}

  @override
  Future<(List<AudioDevice>, List<AudioDevice>)> listDevices() async =>
      (<AudioDevice>[], <AudioDevice>[]);

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
  Future<void> startCall({required Contact contact}) async {}

  @override
  Future<void> startManager() async {}

  @override
  Future<void> startScreenshare({required Contact contact}) async {}

  @override
  Future<void> startSession({required Contact contact}) async {}

  @override
  Future<void> stopSession({required Contact contact}) async {}
}
