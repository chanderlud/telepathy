import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:telepathy/app.dart';
import 'package:telepathy/core/utils/index.dart';
import 'package:telepathy/src/rust/audio/player.dart';
import 'package:telepathy/src/rust/flutter.dart';

Future<Uint8List> _readFileBytes(String path) async {
  final file = File(path);
  return await file.readAsBytes();
}

/// Manages the state of chat messages and attachments.
class ChatStateController extends ChangeNotifier {
  static const int _maxCachedImages = 50;

  /// a list of messages in the chat
  List<ChatMessage> messages = [];

  /// a list of attachments to be sent with the next message
  List<(String, Uint8List)> attachments = [];

  /// a flag indicating if the chat is active and should be enabled
  bool active = false;

  /// the input field for the chat
  TextEditingController messageInput = TextEditingController();

  /// a list of files used in the chat which optionally display images
  Map<String, (File?, Image?)> files = {};

  final List<String> _imageKeys = [];

  final SoundPlayer soundPlayer;

  ChatStateController(this.soundPlayer);

  /// called when a new chat message is received by the backend
  void messageReceived(ChatMessage message) async {
    messages.add(message);

    // handle any attachments
    for (var attachment in message.attachments()) {
      if (kIsWeb) {
        await saveToDownloads(attachment.$2, attachment.$1);
        _addFile(attachment.$1, null, attachment.$2);
      } else {
        File? file = await saveToDownloads(attachment.$2, attachment.$1);
        if (file == null) {
          continue;
        }

        // add the file record
        _addFile(attachment.$1, file, attachment.$2);
      }
    }

    // remove attachment data from memory
    message.clearAttachments();
    notifyListeners();

    // TODO there is no message received sound asset
    // // play the received sound
    // otherSoundHandle = await soundPlayer.play(bytes: await readSeaBytes(''));
  }

  /// adds a file to the list of attachments
  void addAttachmentFile(String name, File file) async {
    if (kIsWeb) {
      final context = navigatorKey.currentState?.context;
      if (context != null && navigatorKey.currentState!.mounted) {
        showErrorDialog(
          context,
          'Attachments not supported on web',
          'Uploading files from disk is not supported in the web build.',
        );
      }
      return;
    }

    final lastDot = name.lastIndexOf('.');
    final nowMs = DateTime.now().millisecondsSinceEpoch;

    final String newName;
    if (lastDot == -1) {
      // No extension; keep the original name and append the timestamp suffix.
      newName = '$name-$nowMs';
    } else {
      final fileNameWithoutExtension = name.substring(0, lastDot);
      final fileExtension = name.substring(lastDot);
      newName = '$fileNameWithoutExtension-$nowMs$fileExtension';
    }

    Uint8List bytes;
    try {
      bytes = await compute(_readFileBytes, file.path);
    } catch (e) {
      if (navigatorKey.currentState != null &&
          navigatorKey.currentState!.mounted) {
        showErrorDialog(
          navigatorKey.currentState!.context,
          'Failed to load attachment',
          e.toString(),
        );
      }
      return;
    }
    attachments.add((newName, bytes));
    _addFile(newName, file, bytes);
    notifyListeners();
  }

  /// adds an attachment from memory to the list of attachments
  void addAttachmentMemory(String name, Uint8List data) {
    final lastDot = name.lastIndexOf('.');
    final nowMs = DateTime.now().millisecondsSinceEpoch;

    final String newName;
    if (lastDot == -1) {
      // No extension; keep the original name and append the timestamp suffix.
      newName = '$name-$nowMs';
    } else {
      final fileNameWithoutExtension = name.substring(0, lastDot);
      final fileExtension = name.substring(lastDot);
      newName = '$fileNameWithoutExtension-$nowMs$fileExtension';
    }

    attachments.add((newName, data));
    _addFile(newName, null, data);
    notifyListeners();
  }

  /// adds a file to the list of files, optionally displaying images
  void _addFile(String name, File? file, Uint8List data) {
    if (isValidImageFormat(name)) {
      Image? image = Image.memory(
        data,
        fit: BoxFit.contain,
        cacheWidth: 800,
        cacheHeight: 800,
        filterQuality: FilterQuality.medium,
      );
      files[name] = (file, image);

      _imageKeys.remove(name);
      _imageKeys.add(name);
      if (_imageKeys.length > _maxCachedImages) {
        final oldestKey = _imageKeys.removeAt(0);
        files.remove(oldestKey);
      }
    } else {
      files[name] = (file, null);
    }
  }

  /// clears the state of the chat
  void clearState() {
    messages.clear();
    attachments.clear();
    messageInput.clear();
    files.clear();
    _imageKeys.clear();
    notifyListeners();
  }

  /// clears the input field and attachments
  void clearInput() {
    messageInput.clear();
    attachments.clear();
    notifyListeners();
  }

  bool isValidImageFormat(String fileName) {
    const validExtensions = ['jpg', 'jpeg', 'png', 'gif', 'bmp', 'webp'];
    final extension = fileName.split('.').last.toLowerCase();
    return validExtensions.contains(extension);
  }

  /// removes an attachment before being sent
  void removeAttachment(String name) {
    attachments.removeWhere((attachment) => attachment.$1 == name);
    files.remove(name);
    _imageKeys.remove(name);
    notifyListeners();
  }
}
