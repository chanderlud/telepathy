import 'dart:convert';
import 'dart:ui';

import 'package:flutter/foundation.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:telepathy/core/constants/network_constants.dart';
import 'package:telepathy/core/constants/overlay_constants.dart';
import 'package:telepathy/core/utils/console.dart';
import 'package:telepathy/models/index.dart';
import 'package:telepathy/src/rust/error.dart';
import 'package:telepathy/src/rust/flutter.dart';

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
    try {
      return NetworkConfig(
        relayAddress:
            await options.getString('relayAddress') ?? defaultRelayAddress,
        relayId: await options.getString('relayId') ?? defaultRelayId,
      );
    } on DartError catch (e) {
      DebugConsole.warn('invalid network config values: $e');
      return NetworkConfig(
          relayAddress: defaultRelayAddress, relayId: defaultRelayId);
    }
  }

  Future<void> saveNetworkConfig() async {
    await options.setString(
        'relayAddress', await networkConfig.getRelayAddress());
    await options.setString('relayId', await networkConfig.getRelayId());
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
    final num clamped = residualBits.clamp(1.0, 8.0);
    codecConfig.setResidualBits(residualBits: clamped.toDouble());
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
