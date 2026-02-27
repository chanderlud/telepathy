import 'package:telepathy/core/utils/console.dart';

class TelepathyShellRunner {
  Future<void> run(String command) async {
    DebugConsole.warn('Shell commands are not supported on web');
  }
}
