import 'dart:core';
import 'package:flutter/material.dart' hide Overlay;
import 'package:provider/provider.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/widgets/common/index.dart';

class InterfaceSettings extends StatefulWidget {
  final BoxConstraints constraints;

  const InterfaceSettings({super.key, required this.constraints});

  @override
  InterfaceSettingsState createState() => InterfaceSettingsState();
}

class InterfaceSettingsState extends State<InterfaceSettings> {
  final TextEditingController _primaryColorInput = TextEditingController();
  String? _primaryColorError;
  late final InterfaceController _controller;

  @override
  void initState() {
    super.initState();
    _controller = context.read<InterfaceController>();
    _primaryColorInput.text = '#${_controller.primaryColor.toRadixString(16)}';
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
            ],
          ),
        ),
      ],
    );
  }
}
