import 'dart:core';
import 'package:telepathy/settings/controller.dart';
import 'package:flutter/material.dart' hide Overlay;
import 'package:telepathy/main.dart';

class InterfaceSettings extends StatefulWidget {
  final InterfaceController controller;
  final BoxConstraints constraints;

  const InterfaceSettings(
      {super.key, required this.controller, required this.constraints});

  @override
  InterfaceSettingsState createState() => InterfaceSettingsState();
}

class InterfaceSettingsState extends State<InterfaceSettings> {
  final TextEditingController _primaryColorInput = TextEditingController();
  String? _primaryColorError;

  @override
  void initState() {
    super.initState();
    _primaryColorInput.text =
    "#${widget.controller.primaryColor.toRadixString(16)}";
  }

  @override
  Widget build(BuildContext context) {
    double width = widget.constraints.maxWidth < 650
        ? widget.constraints.maxWidth
        : (widget.constraints.maxWidth - 20) / 2;

    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
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
                        widget.controller.setPrimaryColor(color);
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
                  widget.controller.setPrimaryColor(0xff5538e5);
                  _primaryColorInput.text = '#ff5538e5';
                },
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