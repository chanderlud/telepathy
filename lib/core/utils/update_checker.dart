import 'dart:async';
import 'dart:convert';

import 'package:http/http.dart' as http;
import 'package:package_info_plus/package_info_plus.dart';
import 'package:telepathy/core/utils/console.dart';

class AvailableUpdate {
  final String version;
  final Uri releaseUrl;

  const AvailableUpdate({required this.version, required this.releaseUrl});
}

class UpdateCheckResult {
  final AvailableUpdate? availableUpdate;
  final String? error;

  const UpdateCheckResult._({this.availableUpdate, this.error});

  const UpdateCheckResult.upToDate() : this._();

  const UpdateCheckResult.updateAvailable(AvailableUpdate update)
      : this._(availableUpdate: update);

  const UpdateCheckResult.failed(String message) : this._(error: message);

  bool get failed => error != null;
}

class UpdateChecker {
  final http.Client? _client;
  final Future<String> Function() _installedVersion;

  static final Uri _latestReleaseUrl = Uri.parse(
    'https://api.github.com/repos/chanderlud/telepathy/releases/latest',
  );

  UpdateChecker({
    http.Client? client,
    Future<String> Function()? installedVersion,
  })  : _client = client,
        _installedVersion = installedVersion ?? _loadInstalledVersion;

  Future<UpdateCheckResult> check() async {
    final client = _client ?? http.Client();

    try {
      final installedVersion = await _installedVersion();
      final response = await client.get(
        _latestReleaseUrl,
        headers: const {
          'Accept': 'application/vnd.github+json',
          'User-Agent': 'Telepathy-Update-Checker',
          'X-GitHub-Api-Version': '2022-11-28',
        },
      ).timeout(const Duration(seconds: 10));

      if (response.statusCode != 200) {
        return _failure('GitHub API returned HTTP ${response.statusCode}');
      }

      final body = jsonDecode(response.body);
      if (body is! Map<String, dynamic>) {
        return _failure('GitHub API returned an unexpected response');
      }

      final tagName = body['tag_name'];
      final releaseUrl = body['html_url'];
      if (tagName is! String || releaseUrl is! String) {
        return _failure('GitHub release data is missing required fields');
      }

      final parsedReleaseUrl = Uri.tryParse(releaseUrl);
      if (parsedReleaseUrl == null ||
          !parsedReleaseUrl.hasScheme ||
          !parsedReleaseUrl.hasAuthority) {
        return _failure('GitHub release URL is invalid');
      }

      if (!isNewerVersion(tagName, installedVersion)) {
        return const UpdateCheckResult.upToDate();
      }

      return UpdateCheckResult.updateAvailable(
        AvailableUpdate(version: tagName, releaseUrl: parsedReleaseUrl),
      );
    } catch (error) {
      return _failure('Update check failed: $error');
    } finally {
      if (_client == null) {
        client.close();
      }
    }
  }

  static bool isNewerVersion(String latest, String installed) {
    final latestParts = _versionParts(latest);
    final installedParts = _versionParts(installed);

    for (var index = 0; index < 3; index++) {
      if (latestParts[index] != installedParts[index]) {
        return latestParts[index] > installedParts[index];
      }
    }

    return false;
  }

  static List<int> _versionParts(String version) {
    final normalized = version.startsWith('v') || version.startsWith('V')
        ? version.substring(1)
        : version;
    final segments = normalized.split('.');

    return List<int>.generate(3, (index) {
      if (index >= segments.length) {
        return 0;
      }

      final numericPrefix = RegExp(r'^\d+').firstMatch(segments[index]);
      return int.tryParse(numericPrefix?.group(0) ?? '') ?? 0;
    });
  }

  UpdateCheckResult _failure(String message) {
    DebugConsole.warn(message);
    return UpdateCheckResult.failed(message);
  }
}

Future<String> _loadInstalledVersion() async {
  final packageInfo = await PackageInfo.fromPlatform();
  return packageInfo.version;
}
