import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:super_clipboard/super_clipboard.dart';
import 'package:telepathy/app.dart';
import 'package:telepathy/controllers/chat_controller.dart';
import 'package:telepathy/core/utils/index.dart';

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

  Future<void> chooseFile() async {
    try {
      final result = await FilePicker.platform.pickFiles(
        withData: Platform.isAndroid || Platform.isIOS,
      );
      if (result == null) return;

      final filePickerFile = result.files.single;
      final name = filePickerFile.name;

      // Handle mobile platforms where path may be null
      if (Platform.isAndroid || Platform.isIOS) {
        // On mobile, use bytes directly if path is null
        if (filePickerFile.path == null) {
          if (filePickerFile.bytes == null) {
            final navigatorState = navigatorKey.currentState;
            if (navigatorState != null && navigatorState.mounted) {
              final context = navigatorState.context;
              if (!context.mounted) return;
              showErrorDialog(
                context,
                'Failed to load file',
                Platform.isIOS
                    ? 'Unable to access file on iOS. Please try selecting the file again.'
                    : 'Unable to access file on Android. Please try selecting the file again.',
              );
            }
            return;
          }

          // Use bytes directly on mobile
          final bytes = filePickerFile.bytes!;

          // File size validation (50MB limit for mobile)
          const maxFileSize = 50 * 1024 * 1024; // 50MB
          if (bytes.length > maxFileSize) {
            final navigatorState = navigatorKey.currentState;
            if (navigatorState != null && navigatorState.mounted) {
              final context = navigatorState.context;
              if (!context.mounted) return;
              showErrorDialog(
                context,
                'File too large',
                'The selected file exceeds the maximum size limit of 50MB.',
              );
            }
            return;
          }

          chatStateController.addAttachmentMemory(name, bytes);
          return;
        }
      }

      // Desktop path-based access
      final path = filePickerFile.path;
      if (path == null) {
        final navigatorState = navigatorKey.currentState;
        if (navigatorState != null && navigatorState.mounted) {
          final context = navigatorState.context;
          if (!context.mounted) return;
          showErrorDialog(
            context,
            'Failed to load file',
            'Unable to access file path. Please try selecting the file again.',
          );
        }
        return;
      }

      final file = File(path);
      chatStateController.addAttachmentFile(name, file);
    } catch (e) {
      DebugConsole.error('File picker error: $e');
      final navigatorState = navigatorKey.currentState;
      if (navigatorState != null && navigatorState.mounted) {
        final context = navigatorState.context;
        if (!context.mounted) return;
        showErrorDialog(
          context,
          'File selection failed',
          'An error occurred while selecting the file: ${e.toString()}',
        );
      }
    }
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
      try {
        final formats = item.getFormats(Formats.standardFormats);
        final suggestedName = await item.getSuggestedName();

        for (final format in formats) {
          // Plain text is already handled by the text field.
          if (format == Formats.plainText) continue;

          // Handle image formats: PNG, JPEG, GIF, WebP, BMP, TIFF
          if (format == Formats.png ||
              format == Formats.jpeg ||
              format == Formats.gif ||
              format == Formats.webp ||
              format == Formats.bmp ||
              format == Formats.tiff) {
            try {
              final bytes = await item.readFile(format as FileFormat);
              if (bytes == null) continue;

              // File size validation (50MB limit)
              const maxFileSize = 50 * 1024 * 1024; // 50MB
              if (bytes.length > maxFileSize) {
                final navigatorState = navigatorKey.currentState;
                if (navigatorState != null && navigatorState.mounted) {
                  final context = navigatorState.context;
                  if (!context.mounted) return;
                  showErrorDialog(
                    context,
                    'File too large',
                    'The pasted file exceeds the maximum size limit of 50MB.',
                  );
                }
                continue;
              }

              // Generate appropriate filename based on format
              String fileName;
              if (suggestedName != null && suggestedName.isNotEmpty) {
                fileName = suggestedName;
              } else {
                // Generate name based on format type
                final extension = _getExtensionForFormat(format);
                fileName = 'pasted-image.$extension';
              }

              chatStateController.addAttachmentMemory(fileName, bytes);
            } catch (e) {
              DebugConsole.error(
                  'Failed to read clipboard file format $format: $e');
              // Continue to next format instead of failing completely
              continue;
            }
          }
        }
      } catch (e) {
        DebugConsole.error('Failed to process clipboard item: $e');
        // Continue to next item instead of failing completely
        continue;
      }
    }
  }

  /// Returns the file extension for a given clipboard format
  String _getExtensionForFormat(DataFormat format) {
    if (format == Formats.png) return 'png';
    if (format == Formats.jpeg) return 'jpg';
    if (format == Formats.gif) return 'gif';
    if (format == Formats.webp) return 'webp';
    if (format == Formats.bmp) return 'bmp';
    if (format == Formats.tiff) return 'tiff';
    return 'png'; // Default fallback
  }
}
