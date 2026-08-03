export 'dart:io';
import 'dart:io' show Platform;

/// True on the desktop platforms supported by window_manager.
bool get isDesktopPlatform =>
    Platform.isWindows || Platform.isMacOS || Platform.isLinux;
