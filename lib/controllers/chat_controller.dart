import 'dart:io';
import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:telepathy/core/utils/file_utils.dart';
import 'package:telepathy/src/rust/audio/player.dart';
import 'package:telepathy/src/rust/flutter.dart';

/// Manages the state of chat messages and attachments.
class ChatStateController extends ChangeNotifier {
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

  final SoundPlayer soundPlayer;

  ChatStateController(this.soundPlayer);

  /// called when a new chat message is received by the backend
  void messageReceived(ChatMessage message) async {
    messages.add(message);

    // handle any attachments
    for (var attachment in message.attachments()) {
      File? file = await saveToDownloads(attachment.$2, attachment.$1);

      if (file == null) {
        continue;
      }

      // add the file record
      _addFile(attachment.$1, file, attachment.$2);
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
    final fileNameWithoutExtension = name.substring(0, name.lastIndexOf('.'));
    final fileExtension = name.substring(name.lastIndexOf('.'));
    String newName =
        '$fileNameWithoutExtension-${DateTime.now().millisecondsSinceEpoch}$fileExtension';

    Uint8List bytes = await file.readAsBytes();
    attachments.add((newName, bytes));
    _addFile(newName, file, bytes);
    notifyListeners();
  }

  /// adds an attachment from memory to the list of attachments
  void addAttachmentMemory(String name, Uint8List data) {
    final fileNameWithoutExtension = name.substring(0, name.lastIndexOf('.'));
    final fileExtension = name.substring(name.lastIndexOf('.'));
    String newName =
        '$fileNameWithoutExtension-${DateTime.now().millisecondsSinceEpoch}$fileExtension';

    attachments.add((newName, data));
    _addFile(newName, null, data);
    notifyListeners();
  }

  /// adds a file to the list of files, optionally displaying images
  void _addFile(String name, File? file, Uint8List data) {
    if (isValidImageFormat(name)) {
      Image? image = Image.memory(data, fit: BoxFit.contain);
      files[name] = (file, image);
    } else {
      files[name] = (file, null);
    }
  }

  /// clears the state of the chat
  void clearState() {
    messages.clear();
    attachments.clear();
    messageInput.clear();
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
    notifyListeners();
  }
}

