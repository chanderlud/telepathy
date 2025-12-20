import 'package:process_run/process_run.dart';

class TelepathyShellRunner {
  final Shell _shell = Shell();

  Future<void> run(String command) async {
    await _shell.run(command);
  }
}
