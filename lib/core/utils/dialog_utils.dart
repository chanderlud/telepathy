import 'dart:async';

import 'package:flutter/material.dart';
import 'package:telepathy/src/rust/flutter.dart';
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
Future<bool> acceptCallPrompt(BuildContext context, Contact contact) async {
  const timeout = Duration(seconds: 10);

  if (!context.mounted) {
    return false;
  }

  Timer? timeoutTimer;
  bool? result = await showDialog<bool>(
    context: context,
    barrierDismissible: false,
    builder: (BuildContext context) {
      timeoutTimer = Timer(timeout, () {
        if (context.mounted) {
          Navigator.of(context).pop(false);
        }
      });

      return AlertDialog(
        title: Text('Accept call from ${contact.nickname()}?'),
        actions: <Widget>[
          TextButton(
            child: const Text('Deny'),
            onPressed: () {
              timeoutTimer?.cancel();
              Navigator.of(context).pop(false);
            },
          ),
          TextButton(
            child: const Text('Accept'),
            onPressed: () {
              timeoutTimer?.cancel();
              Navigator.of(context).pop(true);
            },
          ),
        ],
      );
    },
  );

  return result ?? false;
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
