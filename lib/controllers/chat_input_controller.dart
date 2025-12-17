import 'dart:io';

import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:super_clipboard/super_clipboard.dart';
import 'package:telepathy/controllers/chat_controller.dart';
import 'package:telepathy/core/utils/clipboard_extensions.dart';
import 'package:telepathy/core/utils/console.dart';

/// Handles chat input side-effects (paste + file pick) so the UI can stay focused
/// on composition.
class ChatInputController {
  final ChatStateController chatStateController;
  final FocusNode focusNode;

  bool _keyboardHandlerAttached = false;

  ChatInputController({
    required this.chatStateController,
    required this.focusNode,
  });

  void init() {
    ClipboardEvents.instance?.registerPasteEventListener(_onPasteEvent);
    focusNode.addListener(_onFocusChanged);
  }

  void dispose() {
    ClipboardEvents.instance?.unregisterPasteEventListener(_onPasteEvent);
    focusNode.removeListener(_onFocusChanged);

    if (_keyboardHandlerAttached) {
      HardwareKeyboard.instance.removeHandler(_onKeyEvent);
      _keyboardHandlerAttached = false;
    }
  }

  // TODO mobile compatibility (file picker + clipboard behavior)
  Future<void> chooseFile() async {
    final result = await FilePicker.platform.pickFiles();
    if (result == null) return;

    final path = result.files.single.path;
    if (path == null) return;

    final file = File(path);
    final name = result.files.single.name;
    chatStateController.addAttachmentFile(name, file);
  }

  Future<void> _onPasteEvent(ClipboardReadEvent event) async {
    final reader = await event.getClipboardReader();
    await handlePaste(reader);
  }

  void _onFocusChanged() {
    if (focusNode.hasFocus && !_keyboardHandlerAttached) {
      HardwareKeyboard.instance.addHandler(_onKeyEvent);
      _keyboardHandlerAttached = true;
    } else if (!focusNode.hasFocus && _keyboardHandlerAttached) {
      HardwareKeyboard.instance.removeHandler(_onKeyEvent);
      _keyboardHandlerAttached = false;
    }
  }

  bool _onKeyEvent(KeyEvent event) {
    if (event is! KeyDownEvent) return false;

    if (HardwareKeyboard.instance.isControlPressed &&
        event.logicalKey == LogicalKeyboardKey.keyV) {
      final clipboard = SystemClipboard.instance;
      if (clipboard == null) {
        DebugConsole.debug('Clipboard is null');
        return false;
      }

      clipboard.read().then(handlePaste);
      return true;
    }

    return false;
  }

  Future<void> handlePaste(ClipboardReader reader) async {
    for (final item in reader.items) {
      final formats = item.getFormats(Formats.standardFormats);
      final suggestedName = await item.getSuggestedName();

      for (final format in formats) {
        // Plain text is already handled by the text field.
        if (format == Formats.plainText) continue;

        // TODO handle more formats
        if (format == Formats.png || format == Formats.jpeg) {
          final bytes = await item.readFile(format as FileFormat);
          if (bytes == null) continue;

          chatStateController.addAttachmentMemory(
            suggestedName ?? 'pasted-image.png',
            bytes,
          );
        }
      }
    }
  }
}
