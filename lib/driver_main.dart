import 'package:flutter_driver/driver_extension.dart';
import 'package:telepathy/main.dart' as app;

/// Entrypoint for automated QA runs (see scripts/run-linux-debug.sh) that
/// enables the flutter_driver extension before booting the real app.
void main(List<String> args) {
  enableFlutterDriverExtension();
  app.main(args);
}
