import 'dart:core';
import 'package:flutter/material.dart' hide Overlay;
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/src/rust/telepathy.dart';
import 'package:telepathy/src/rust/error.dart';
import 'package:telepathy/widgets/common/index.dart';

class NetworkSettings extends StatefulWidget {
  final NetworkSettingsController networkSettingsController;
  final Telepathy telepathy;
  final StateController stateController;
  final BoxConstraints constraints;

  const NetworkSettings(
      {super.key,
      required this.networkSettingsController,
      required this.telepathy,
      required this.stateController,
      required this.constraints});

  @override
  NetworkSettingsState createState() => NetworkSettingsState();
}

class NetworkSettingsState extends State<NetworkSettings> {
  late String _relayAddress;
  late String _relayPeerId;
  bool unsavedChanges = false;

  final TextEditingController _relayAddressInput = TextEditingController();
  String? _relayAddressError;

  final TextEditingController _relayPeerIdInput = TextEditingController();
  String? _relayPeerIdError;

  @override
  void initState() {
    super.initState();
    _initialize();
  }

  Future<void> _initialize() async {
    _relayAddress =
        await widget.networkSettingsController.networkConfig.getRelayAddress();
    _relayPeerId =
        await widget.networkSettingsController.networkConfig.getRelayId();

    _relayAddressInput.text = _relayAddress;
    _relayPeerIdInput.text = _relayPeerId;
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
                    labelText: 'Relay Address',
                    controller: _relayAddressInput,
                    onChanged: (String value) {
                      if (value != _relayAddress) {
                        setState(() {
                          unsavedChanges = true;
                        });
                      }
                    },
                    error: _relayAddressError == null
                        ? null
                        : Text(_relayAddressError!,
                            style: const TextStyle(color: Colors.red)),
                  )),
              SizedBox(
                  width: width,
                  child: TextInput(
                    labelText: 'Relay Peer ID',
                    controller: _relayPeerIdInput,
                    onChanged: (String value) {
                      if (value != _relayPeerId) {
                        setState(() {
                          unsavedChanges = true;
                        });
                      }
                    },
                    error: _relayPeerIdError == null
                        ? null
                        : Text(_relayPeerIdError!,
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

  Future<void> saveChanges() async {
    String relayAddress = _relayAddressInput.text;
    String relayId = _relayPeerIdInput.text;

    bool changed = false;

    try {
      // this will raise an error if the relay ID isn't formatted right
      await widget.networkSettingsController.networkConfig
          .setRelayId(relayId: relayId);
      _relayPeerId = relayId;
      changed = true;
      setState(() {
        _relayPeerIdError = null;
      });
    } on DartError catch (error) {
      setState(() {
        _relayPeerIdError = error.message;
      });
    }

    try {
      // this will raise an error if the relay address isn't a valid socket address
      await widget.networkSettingsController.networkConfig
          .setRelayAddress(relayAddress: relayAddress);
      _relayAddress = relayAddress;
      changed = true;
      setState(() {
        _relayAddressError = null;
      });
    } on DartError catch (error) {
      setState(() {
        _relayAddressError = error.message;
      });
    }

    unsavedChanges = _relayAddressError != null || _relayPeerIdError != null;

    if (changed) {
      widget.networkSettingsController.saveNetworkConfig();
      widget.telepathy.restartManager();
    }
  }
}
