import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:telepathy/core/utils/update_checker.dart';

void main() {
  group('UpdateChecker.check', () {
    test('returns release details when GitHub has a newer version', () async {
      final checker = UpdateChecker(
        client: MockClient((request) async {
          expect(request.headers['user-agent'], 'Telepathy-Update-Checker');
          return http.Response(
            '{"tag_name":"v2.9.0","html_url":"https://github.com/chanderlud/telepathy/releases/tag/v2.9.0"}',
            200,
          );
        }),
        installedVersion: () async => '2.8.1',
      );

      final result = await checker.check();

      expect(result.failed, isFalse);
      expect(result.availableUpdate?.version, 'v2.9.0');
      expect(
        result.availableUpdate?.releaseUrl,
        Uri.parse(
          'https://github.com/chanderlud/telepathy/releases/tag/v2.9.0',
        ),
      );
    });

    test('returns up to date when installed version matches', () async {
      final checker = UpdateChecker(
        client: MockClient(
          (_) async => http.Response(
            '{"tag_name":"v2.8.1","html_url":"https://github.com/chanderlud/telepathy/releases/tag/v2.8.1"}',
            200,
          ),
        ),
        installedVersion: () async => '2.8.1',
      );

      final result = await checker.check();

      expect(result.failed, isFalse);
      expect(result.availableUpdate, isNull);
    });

    test('returns failure for a GitHub API error', () async {
      final checker = UpdateChecker(
        client: MockClient((_) async => http.Response('rate limited', 403)),
        installedVersion: () async => '2.8.1',
      );

      final result = await checker.check();

      expect(result.failed, isTrue);
      expect(result.availableUpdate, isNull);
    });

    test('returns failure for malformed release data', () async {
      final checker = UpdateChecker(
        client:
            MockClient((_) async => http.Response('{"name":"Latest"}', 200)),
        installedVersion: () async => '2.8.1',
      );

      final result = await checker.check();

      expect(result.failed, isTrue);
      expect(result.availableUpdate, isNull);
    });
  });

  group('UpdateChecker.isNewerVersion', () {
    test('detects newer major, minor, and patch versions', () {
      expect(UpdateChecker.isNewerVersion('v3.0.0', '2.8.1'), isTrue);
      expect(UpdateChecker.isNewerVersion('v2.9.0', '2.8.1'), isTrue);
      expect(UpdateChecker.isNewerVersion('v2.8.2', '2.8.1'), isTrue);
    });

    test('does not report equal or older versions as newer', () {
      expect(UpdateChecker.isNewerVersion('v2.8.1', '2.8.1'), isFalse);
      expect(UpdateChecker.isNewerVersion('v2.8.0', '2.8.1'), isFalse);
      expect(UpdateChecker.isNewerVersion('v1.12.9', '2.0.0'), isFalse);
    });

    test('pads missing segments and handles odd segments', () {
      expect(UpdateChecker.isNewerVersion('v2.9', '2.8.1'), isTrue);
      expect(UpdateChecker.isNewerVersion('2.8', '2.8.0'), isFalse);
      expect(UpdateChecker.isNewerVersion('v2.8.2-beta.1', '2.8.1'), isTrue);
      expect(UpdateChecker.isNewerVersion('release', '0.0.0'), isFalse);
    });
  });
}
