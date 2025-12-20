import 'dart:typed_data';

/// Minimal `dart:io` shims for platforms where `dart:io` is unavailable (web).
///
/// These exist to keep shared code compiling; runtime use of these on web
/// should be guarded
class Platform {
  static bool get isWindows => false;
  static bool get isMacOS => false;
  static bool get isLinux => false;
  static bool get isAndroid => false;
  static bool get isIOS => false;
}

class Directory {
  final String path;
  Directory(this.path);
}

class File {
  final String path;
  File(this.path);

  Directory get parent => Directory(_dirname(path));

  Future<Uint8List> readAsBytes() {
    throw UnsupportedError('File.readAsBytes is not supported on web');
  }
}

String _dirname(String p) {
  final normalized = p.replaceAll('\\', '/');
  final idx = normalized.lastIndexOf('/');
  if (idx <= 0) return '';
  return normalized.substring(0, idx);
}
