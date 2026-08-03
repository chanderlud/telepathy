import 'dart:core';

import 'package:flutter/material.dart' hide Overlay;
import 'package:provider/provider.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/core/utils/index.dart';
import 'package:telepathy/widgets/common/index.dart';

class GeneralSettings extends StatefulWidget {
  final BoxConstraints constraints;

  const GeneralSettings({super.key, required this.constraints});

  @override
  GeneralSettingsState createState() => GeneralSettingsState();
}

class GeneralSettingsState extends State<GeneralSettings> {
  final TextEditingController _primaryColorInput = TextEditingController();
  String? _primaryColorError;
  late final InterfaceController _controller;
  bool _checkingForUpdates = false;

  @override
  void initState() {
    super.initState();
    _controller = context.read<InterfaceController>();
    _primaryColorInput.text = '#${_controller.primaryColor.toRadixString(16)}';
  }

  @override
  void dispose() {
    _primaryColorInput.dispose();
    super.dispose();
  }

  Future<void> _checkForUpdates() async {
    setState(() {
      _checkingForUpdates = true;
    });

    final result = await UpdateChecker().check();
    if (!mounted) {
      return;
    }

    setState(() {
      _checkingForUpdates = false;
    });

    final update = result.availableUpdate;
    if (update != null) {
      await showUpdateAvailableDialog(context, update);
      return;
    }

    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(
          result.failed
              ? 'Could not check for updates. Please try again.'
              : 'Telepathy is up to date.',
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final preferencesController = context.watch<PreferencesController>();
    double width = widget.constraints.maxWidth < 650
        ? widget.constraints.maxWidth
        : (widget.constraints.maxWidth - 20) / 2;

    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const Text(
          'General',
          style: TextStyle(fontSize: 20),
        ),
        const SizedBox(height: 17),
        Center(
          child: Wrap(
            spacing: 20,
            runSpacing: 20,
            children: [
              SizedBox(
                  width: width,
                  child: TextInput(
                    labelText: 'Primary Color',
                    controller: _primaryColorInput,
                    onChanged: (String value) {
                      int? color =
                          int.tryParse(value.replaceAll('#', ''), radix: 16);

                      if (color == null) {
                        _primaryColorError = 'Invalid hex color';
                      } else {
                        _primaryColorError = null;
                        _controller.setPrimaryColor(color);
                      }
                    },
                    error: _primaryColorError == null
                        ? null
                        : Text(_primaryColorError!,
                            style: const TextStyle(color: Colors.red)),
                  )),
              Button(
                text: 'Revert primary color to default',
                onPressed: () {
                  _controller.setPrimaryColor(0xff5538e5);
                  _primaryColorInput.text = '#ff5538e5';
                },
                width: 200,
                height: 25,
              ),
              SizedBox(
                width: width,
                child: Row(
                  mainAxisAlignment: MainAxisAlignment.spaceBetween,
                  children: [
                    const Expanded(
                      child: Text(
                        'Check for updates automatically',
                        style: TextStyle(fontSize: 18),
                      ),
                    ),
                    CustomSwitch(
                      value: preferencesController.automaticUpdateChecks,
                      onChanged:
                          preferencesController.updateAutomaticUpdateChecks,
                    ),
                  ],
                ),
              ),
              Button(
                text: _checkingForUpdates
                    ? 'Checking for updates...'
                    : 'Check for updates',
                disabled: _checkingForUpdates,
                onPressed: _checkForUpdates,
                width: 200,
                height: 25,
              ),
            ],
          ),
        ),
      ],
    );
  }
}
