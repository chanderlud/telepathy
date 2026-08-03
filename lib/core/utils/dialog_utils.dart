import 'dart:async';

import 'package:flutter/material.dart';
import 'package:telepathy/core/rust/types.dart';
import 'package:telepathy/widgets/common/index.dart';

/// Shows an error modal.
void showErrorDialog(BuildContext context, String title, String errorMessage) {
  showDialog(
    context: context,
    builder: (BuildContext context) {
      return AlertDialog(
        title: Text(title),
        content: Text(errorMessage),
        actions: <Widget>[
          TextButton(
            child: const Text('Close'),
            onPressed: () {
              Navigator.of(context).pop();
            },
          ),
        ],
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(10),
        ),
      );
    },
  );
}

/// Prompts the user to accept an incoming call.
Future<bool> acceptCallPrompt(
  BuildContext context,
  Contact contact,
  Future<void> cancellation,
) async {
  const timeout = Duration(seconds: 10);

  if (!context.mounted) {
    return false;
  }

  Timer? timeoutTimer;
  bool dialogOpen = true;
  final navigator = Navigator.of(context, rootNavigator: true);
  final themes = InheritedTheme.capture(from: context, to: navigator.context);
  late final DialogRoute<bool> promptRoute;

  void closePrompt(bool result) {
    if (!dialogOpen) {
      return;
    }

    dialogOpen = false;
    timeoutTimer?.cancel();
    navigator.removeRoute(promptRoute, result);
  }

  try {
    promptRoute = DialogRoute<bool>(
      context: context,
      barrierDismissible: false,
      builder: (BuildContext dialogContext) {
        timeoutTimer = Timer(timeout, () => closePrompt(false));
        cancellation.then((_) {
          if (dialogContext.mounted) {
            closePrompt(false);
          }
        });

        return AlertDialog(
          title: Text('Accept call from ${contact.nickname()}?'),
          actions: <Widget>[
            TextButton(
              child: const Text('Deny'),
              onPressed: () => closePrompt(false),
            ),
            TextButton(
              child: const Text('Accept'),
              onPressed: () => closePrompt(true),
            ),
          ],
        );
      },
      themes: themes,
    );
    bool? result = await navigator.push(promptRoute);

    return result ?? false;
  } finally {
    dialogOpen = false;
    timeoutTimer?.cancel();
  }
}

/// Confirms leaving a page with unsaved changes.
Future<bool> unsavedConfirmation(BuildContext context) async {
  bool? result = await showDialog<bool>(
    context: context,
    builder: (BuildContext context) {
      return AlertDialog(
        title: const Text('Unsaved Changes'),
        content: const Text(
            'You have unsaved changes. Are you sure you want to leave?'),
        actions: [
          Button(
            text: 'Cancel',
            onPressed: () {
              Navigator.of(context).pop(false);
            },
          ),
          Button(
            text: 'Leave',
            onPressed: () {
              Navigator.of(context).pop(true);
            },
          )
        ],
      );
    },
  );

  return result ?? false;
}
