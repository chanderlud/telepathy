import 'dart:core';
import 'package:flutter/material.dart' hide Overlay;
import 'package:provider/provider.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/core/rust/flutter.dart';
import 'package:telepathy/core/rust/types.dart';
import 'package:telepathy/widgets/common/index.dart';

class NetworkSettings extends StatefulWidget {
  final BoxConstraints constraints;

  const NetworkSettings({super.key, required this.constraints});

  @override
  NetworkSettingsState createState() => NetworkSettingsState();
}

class NetworkSettingsState extends State<NetworkSettings> {
  late int _listenPort;
  late List<String> _bindAddresses;
  bool unsavedChanges = false;

  final TextEditingController _listenPortInput = TextEditingController();
  String? _listenPortError;

  final TextEditingController _bindAddressesInput = TextEditingController();
  String? _bindAddressesError;

  @override
  void initState() {
    super.initState();
    _initialize();
  }

  Future<void> _initialize() async {
    final networkSettingsController = context.read<NetworkSettingsController>();
    _listenPort = networkSettingsController.networkConfig.getListenPort();
    _bindAddresses = networkSettingsController.networkConfig.getBindAddresses();

    _listenPortInput.text = _listenPort.toString();
    _bindAddressesInput.text = _formatBindAddresses(_bindAddresses);
  }

  @override
  void dispose() {
    _listenPortInput.dispose();
    _bindAddressesInput.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    double width = widget.constraints.maxWidth < 650
        ? widget.constraints.maxWidth
        : (widget.constraints.maxWidth - 20) / 2;

    return Column(
      mainAxisSize: MainAxisSize.min,
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
                    labelText: 'Bind Addresses',
                    hintText: '0.0.0.0, 127.0.0.1',
                    controller: _bindAddressesInput,
                    onChanged: (_) => _updateUnsavedChanges(),
                    error: _bindAddressesError == null
                        ? null
                        : Text(_bindAddressesError!,
                            style: const TextStyle(color: Colors.red)),
                  )),
              SizedBox(
                  width: width,
                  child: TextInput(
                    labelText: 'Listen Port',
                    controller: _listenPortInput,
                    onChanged: (_) => _updateUnsavedChanges(),
                    error: _listenPortError == null
                        ? null
                        : Text(_listenPortError!,
                            style: const TextStyle(color: Colors.red)),
                  )),
            ],
          ),
        ),
        if (unsavedChanges) const SizedBox(height: 20),
        if (unsavedChanges)
          Button(
            text: 'Save Changes',
            onPressed: saveChanges,
            width: 100,
          ),
      ],
    );
  }

  String _formatBindAddresses(List<String> addresses) {
    return addresses.join(', ');
  }

  List<String> _parseBindAddresses(String value) {
    return value
        .split(',')
        .map((address) => address.trim())
        .where((address) => address.isNotEmpty)
        .toList();
  }

  bool _sameBindAddresses(List<String> first, List<String> second) {
    if (first.length != second.length) return false;

    for (int i = 0; i < first.length; i++) {
      if (first[i] != second[i]) return false;
    }

    return true;
  }

  void _updateUnsavedChanges() {
    final int? listenPort = int.tryParse(_listenPortInput.text.trim());
    final List<String> bindAddresses =
        _parseBindAddresses(_bindAddressesInput.text);

    setState(() {
      unsavedChanges = listenPort != _listenPort ||
          !_sameBindAddresses(bindAddresses, _bindAddresses);
    });
  }

  Future<void> saveChanges() async {
    final String listenPortText = _listenPortInput.text.trim();
    final int? listenPort = int.tryParse(listenPortText);
    final List<String> bindAddresses =
        _parseBindAddresses(_bindAddressesInput.text);

    String? listenPortError;
    String? bindAddressesError;

    if (listenPort == null) {
      listenPortError = 'Listen port must be a number';
    } else if (listenPort < 0 || listenPort > 65535) {
      listenPortError = 'Listen port must be between 0 and 65535';
    }

    if (bindAddresses.isEmpty) {
      bindAddressesError = 'Enter at least one bind address';
    }

    if (listenPortError == null && bindAddressesError == null) {
      try {
        NetworkConfig(
          listenPort: listenPort!,
          bindAddresses: bindAddresses,
        );
      } on DartError catch (error) {
        bindAddressesError = error.message;
      }
    }

    if (listenPortError != null || bindAddressesError != null) {
      setState(() {
        _listenPortError = listenPortError;
        _bindAddressesError = bindAddressesError;
        unsavedChanges = true;
      });

      return;
    }

    final int newListenPort = listenPort!;

    final networkSettingsController = context.read<NetworkSettingsController>();
    final telepathy = context.read<Telepathy>();

    final bool listenPortChanged = newListenPort != _listenPort;
    final bool bindAddressesChanged =
        !_sameBindAddresses(bindAddresses, _bindAddresses);

    if (!listenPortChanged && !bindAddressesChanged) {
      setState(() {
        _listenPortError = null;
        _bindAddressesError = null;
        unsavedChanges = false;
      });

      return;
    }

    try {
      networkSettingsController.networkConfig
          .setListenPort(listenPort: newListenPort);
    } on DartError catch (error) {
      setState(() {
        _listenPortError = error.message;
        _bindAddressesError = null;
        unsavedChanges = true;
      });

      return;
    }

    try {
      networkSettingsController.networkConfig
          .setBindAddresses(bindAddresses: bindAddresses);
    } on DartError catch (error) {
      setState(() {
        _listenPortError = null;
        _bindAddressesError = error.message;
        unsavedChanges = true;
      });

      if (listenPortChanged) {
        networkSettingsController.networkConfig
            .setListenPort(listenPort: _listenPort);
      }

      return;
    }

    await networkSettingsController.saveNetworkConfig();
    await telepathy.restartManager();

    setState(() {
      _listenPort = newListenPort;
      _bindAddresses = bindAddresses;
      _listenPortError = null;
      _bindAddressesError = null;
      unsavedChanges = false;
    });
  }
}
