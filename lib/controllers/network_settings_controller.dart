import 'dart:convert';
import 'dart:ui';

import 'package:flutter/foundation.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:telepathy/core/constants/network_constants.dart';
import 'package:telepathy/core/constants/overlay_constants.dart';
import 'package:telepathy/core/utils/console.dart';
import 'package:telepathy/models/index.dart';
import 'package:telepathy/core/rust/types.dart';

class NetworkSettingsController with ChangeNotifier {
  final SharedPreferencesAsync options;

  /// the network configuration
  late NetworkConfig networkConfig;

  /// the screenshare configuration
  late ScreenshareConfig screenshareConfig;

  /// the overlay configuration
  late OverlayConfig overlayConfig;

  /// the codec configuration
  late CodecConfig codecConfig;

  NetworkSettingsController({required this.options});

  Future<void> init() async {
    networkConfig = await loadNetworkConfig();
    screenshareConfig = await loadScreenshareConfig();
    overlayConfig = await loadOverlayConfig();
    codecConfig = await loadCodecConfig();
    notifyListeners();
  }

  Future<NetworkConfig> loadNetworkConfig() async {
    final StoredNetworkValues stored = await _readStoredNetworkValues();
    return buildNetworkConfigFromSpec(stored: stored);
  }

  @visibleForTesting
  static NetworkConfig buildNetworkConfigFromSpec({
    required StoredNetworkValues stored,
    NetworkConfigBuilder? builder,
  }) {
    return _buildNetworkConfigFromSpec(
      stored: stored,
      builder: builder ?? _constructDefaultNetworkConfig,
    );
  }

  Future<StoredNetworkValues> _readStoredNetworkValues() async {
    return StoredNetworkValues(
      listenPort: await options.getInt('listenPort'),
      bindAddresses: await options.getStringList('bindAddresses'),
      customRelaysEnabled:
          await options.getBool('customRelaysEnabled') ?? false,
      relays: await options.getStringList('relays'),
      customDnsEnabled: await options.getBool('customDnsEnabled') ?? false,
      dnsEndpoint: await options.getString('dnsEndpoint'),
      dnsOriginDomain: await options.getString('dnsOriginDomain'),
      customPkarrEnabled: await options.getBool('customPkarrEnabled') ?? false,
      pkarrRelay: await options.getString('pkarrRelay'),
    );
  }

  static NetworkConfig _buildNetworkConfigFromSpec({
    required StoredNetworkValues stored,
    required NetworkConfigBuilder builder,
  }) {
    try {
      // Construct a defaults-only base first so any per-field rejection below
      // leaves the live config in a known-safe state.
      final NetworkConfig config = builder(
        listenPort: defaultListenPort,
        bindAddresses: defaultBindAddresses,
        relays: null,
        dnsEndpoint: null,
        dnsOriginDomain: null,
        pkarrRelay: null,
      );

      // Apply stored values via the atomic `update` so either every group
      // commits or none does. Groups with empty/half-stored values are
      // dropped here rather than passed as `None`; passing `None` would
      // override the previously-configured value, which is not the same
      // as "skip this group".
      final int newListenPort = stored.listenPort ?? defaultListenPort;
      final List<String> newBindAddresses =
          (stored.bindAddresses != null && stored.bindAddresses!.isNotEmpty)
              ? stored.bindAddresses!
              : defaultBindAddresses;
      final List<String>? newRelays = (stored.customRelaysEnabled &&
              stored.relays != null &&
              stored.relays!.isNotEmpty)
          ? stored.relays
          : null;
      final String? newDnsEndpoint = (stored.customDnsEnabled &&
              stored.dnsEndpoint != null &&
              stored.dnsEndpoint!.isNotEmpty &&
              stored.dnsOriginDomain != null &&
              stored.dnsOriginDomain!.isNotEmpty)
          ? stored.dnsEndpoint
          : null;
      final String? newDnsOriginDomain =
          newDnsEndpoint != null ? stored.dnsOriginDomain : null;
      final String? newPkarrRelay =
          stored.customPkarrEnabled ? stored.pkarrRelay : null;

      try {
        config.update(
          listenPort: newListenPort,
          bindAddresses: newBindAddresses,
          relays: newRelays,
          dnsEndpoint: newDnsEndpoint,
          dnsOriginDomain: newDnsOriginDomain,
          pkarrRelay: newPkarrRelay,
        );
      } on NetworkConfigUpdateError catch (e) {
        DebugConsole.warn(
          'invalid stored network config field ${e.field.name}: ${e.message}, using defaults',
        );
      } on DartError catch (e) {
        DebugConsole.warn('invalid stored network config, using defaults: $e');
      }

      return config;
    } on DartError catch (e) {
      DebugConsole.warn('failed to build network config, using defaults: $e');
      return builder(
        listenPort: defaultListenPort,
        bindAddresses: defaultBindAddresses,
        relays: null,
        dnsEndpoint: null,
        dnsOriginDomain: null,
        pkarrRelay: null,
      );
    }
  }

  static NetworkConfig _constructDefaultNetworkConfig({
    required int listenPort,
    required List<String> bindAddresses,
    required List<String>? relays,
    required String? dnsEndpoint,
    required String? dnsOriginDomain,
    required String? pkarrRelay,
  }) {
    return NetworkConfig(
      listenPort: listenPort,
      bindAddresses: bindAddresses,
      relays: relays,
      dnsEndpoint: dnsEndpoint,
      dnsOriginDomain: dnsOriginDomain,
      pkarrRelay: pkarrRelay,
    );
  }

  Future<void> saveNetworkConfig() async {
    await options.setInt('listenPort', networkConfig.getListenPort());
    await options.setStringList(
        'bindAddresses', networkConfig.getBindAddresses());

    final List<String>? relays = networkConfig.getRelays();
    // An empty custom-relay list would persist `customRelaysEnabled = true`
    // against zero URLs, which on the rust side disables the default map
    // without a replacement. The UI rejects this at save time; this branch
    // is the defense-in-depth fallback for any path that bypasses it.
    final bool customRelaysEnabled = relays != null && relays.isNotEmpty;
    await options.setBool('customRelaysEnabled', customRelaysEnabled);
    if (customRelaysEnabled) {
      await options.setStringList('relays', relays);
    } else {
      await options.remove('relays');
    }

    final String? dnsEndpoint = networkConfig.getDnsEndpoint();
    final String? dnsOriginDomain = networkConfig.getDnsOriginDomain();
    final bool customDnsEnabled = dnsEndpoint != null &&
        dnsEndpoint.isNotEmpty &&
        dnsOriginDomain != null &&
        dnsOriginDomain.isNotEmpty;
    await options.setBool('customDnsEnabled', customDnsEnabled);
    if (customDnsEnabled) {
      await options.setString('dnsEndpoint', dnsEndpoint);
      await options.setString('dnsOriginDomain', dnsOriginDomain);
    } else {
      await options.remove('dnsEndpoint');
      await options.remove('dnsOriginDomain');
    }

    final String? pkarrRelay = networkConfig.getPkarrRelay();
    await options.setBool('customPkarrEnabled', pkarrRelay != null);
    if (pkarrRelay != null) {
      await options.setString('pkarrRelay', pkarrRelay);
    } else {
      await options.remove('pkarrRelay');
    }
  }

  Future<ScreenshareConfig> loadScreenshareConfig() async {
    final buffer = await options.getString('screenshareConfigBuffer');
    return await ScreenshareConfig.newInstance(
      buffer: buffer != null ? base64Decode(buffer) : [],
    );
  }

  Future<void> saveScreenshareConfig() async {
    await options.setString(
        'screenshareConfigBuffer', base64Encode(screenshareConfig.toBytes()));
  }

  Future<CodecConfig> loadCodecConfig() async {
    return CodecConfig(
      enabled: await options.getBool('codecEnabled') ?? true,
      vbr: await options.getBool('codecVbr') ?? true,
      residualBits: await options.getDouble('codecResidualBits') ?? 5.0,
    );
  }

  Future<void> saveCodecConfig() async {
    (bool, bool, double) values = codecConfig.toValues();
    await options.setBool('codecEnabled', values.$1);
    await options.setBool('codecVbr', values.$2);
    await options.setDouble('codecResidualBits', values.$3);
  }

  Future<void> updateCodecEnabled(bool enabled) async {
    codecConfig.setEnabled(enabled: enabled);
    await saveCodecConfig();
    notifyListeners();
  }

  Future<void> updateCodecVbr(bool vbr) async {
    codecConfig.setVbr(vbr: vbr);
    await saveCodecConfig();
    notifyListeners();
  }

  Future<void> updateCodecResidualBits(double residualBits) async {
    final num clamped = residualBits.clamp(2.0, 8.0);
    final double rounded = (clamped.toDouble() * 10).round() / 10;
    codecConfig.setResidualBits(residualBits: rounded);
    await saveCodecConfig();
    notifyListeners();
  }

  Future<OverlayConfig> loadOverlayConfig() async {
    try {
      return OverlayConfig(
        enabled:
            await options.getBool('overlayEnabled') ?? defaultOverlayEnabled,
        x: await options.getDouble('overlayX') ?? defaultOverlayX,
        y: await options.getDouble('overlayY') ?? defaultOverlayY,
        width: await options.getDouble('overlayWidth') ?? defaultOverlayWidth,
        height:
            await options.getDouble('overlayHeight') ?? defaultOverlayHeight,
        fontFamily: await options.getString('overlayFontFamily') ??
            defaultOverlayFontFamily,
        fontColor: Color(await options.getInt('overlayFontColor') ??
            defaultOverlayFontColor),
        fontHeight: await options.getInt('overlayFontHeight') ??
            defaultOverlayFontHeight,
        backgroundColor: Color(await options.getInt('overlayBackgroundColor') ??
            defaultOverlayFontBackgroundColor),
      );
    } on DartError catch (e) {
      DebugConsole.warn('invalid overlay config format: $e');

      return OverlayConfig(
        enabled: defaultOverlayEnabled,
        x: defaultOverlayX,
        y: defaultOverlayY,
        width: defaultOverlayWidth,
        height: defaultOverlayHeight,
        fontFamily: defaultOverlayFontFamily,
        fontColor: const Color(defaultOverlayFontColor),
        fontHeight: defaultOverlayFontHeight,
        backgroundColor: const Color(defaultOverlayFontBackgroundColor),
      );
    }
  }

  Future<void> saveOverlayConfig() async {
    await options.setBool('overlayEnabled', overlayConfig.enabled);
    await options.setDouble('overlayX', overlayConfig.x);
    await options.setDouble('overlayY', overlayConfig.y);
    await options.setDouble('overlayWidth', overlayConfig.width);
    await options.setDouble('overlayHeight', overlayConfig.height);
    await options.setString('overlayFontFamily', overlayConfig.fontFamily);
    await options.setInt(
        'overlayFontColor', overlayConfig.fontColor.toARGB32());
    await options.setInt('overlayFontHeight', overlayConfig.fontHeight);
    await options.setInt(
        'overlayBackgroundColor', overlayConfig.backgroundColor.toARGB32());
  }
}

/// Factory for constructing a [NetworkConfig] from already-validated values.
///
/// The production implementation calls the rust-backed `NetworkConfig(...)`
/// constructor directly. Tests inject a fake that records the values it
/// received and throws [DartError] on invalid input, mirroring the rust-side
/// validation. This is the only seam in the production code that touches
/// rust-bridged types during loading; mocking the rust binding is necessary
/// because the native library is not loaded in `flutter test` and the
/// [NetworkConfig] type is a `RustOpaque` that cannot be subclassed.
typedef NetworkConfigBuilder = NetworkConfig Function({
  required int listenPort,
  required List<String> bindAddresses,
  required List<String>? relays,
  required String? dnsEndpoint,
  required String? dnsOriginDomain,
  required String? pkarrRelay,
});

/// Snapshot of the values pulled from persistent storage during loading.
@visibleForTesting
class StoredNetworkValues {
  const StoredNetworkValues({
    this.listenPort,
    this.bindAddresses,
    this.customRelaysEnabled = false,
    this.relays,
    this.customDnsEnabled = false,
    this.dnsEndpoint,
    this.dnsOriginDomain,
    this.customPkarrEnabled = false,
    this.pkarrRelay,
  });

  final int? listenPort;
  final List<String>? bindAddresses;
  final bool customRelaysEnabled;
  final List<String>? relays;
  final bool customDnsEnabled;
  final String? dnsEndpoint;
  final String? dnsOriginDomain;
  final bool customPkarrEnabled;
  final String? pkarrRelay;
}
