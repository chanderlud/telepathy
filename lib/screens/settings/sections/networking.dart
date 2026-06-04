import 'dart:core';
import 'package:flutter/foundation.dart' show kIsWeb;
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
  bool _saveSucceeded = false;
  bool _isSaving = false;

  final TextEditingController _listenPortInput = TextEditingController();
  String? _listenPortError;

  final TextEditingController _bindAddressesInput = TextEditingController();
  String? _bindAddressesError;

  // Relays
  bool _useCustomRelays = false;
  List<String> _relays = [];
  final TextEditingController _relaysInput = TextEditingController();
  String? _relaysError;

  // DNS
  bool _useCustomDns = false;
  String _dnsEndpoint = '';
  String _dnsOriginDomain = '';
  final TextEditingController _dnsEndpointInput = TextEditingController();
  final TextEditingController _dnsOriginDomainInput = TextEditingController();
  String? _dnsEndpointError;
  String? _dnsOriginDomainError;

  // Pkarr
  bool _useCustomPkarr = false;
  String _pkarrRelay = '';
  final TextEditingController _pkarrRelayInput = TextEditingController();
  String? _pkarrRelayError;

  // Critical backend error (e.g. a poisoned lock from the rust
  // runtime). Not tied to any one field; the rust atomic `update`
  // collapses every poison failure into `NetworkConfigField.backendError`
  // precisely so the frontend can distinguish "your input was bad"
  // (route to the matching field) from "the rust runtime is in a bad
  // state" (route here). Surfacing this inline rather than on a
  // specific input avoids the previous behaviour of painting every
  // backend failure on the bind-addresses field.
  String? _backendError;

  // Saved baseline values for the custom-* toggles. Tracked separately
  // from `_useCustom*` because the dirty check needs to know the *saved*
  // state, not the current in-memory state, and toggling a switch OFF
  // clears the associated list/string fields before the dirty check
  // runs, which would otherwise make the comparison see "no change".
  bool _savedUseCustomRelays = false;
  bool _savedUseCustomDns = false;
  bool _savedUseCustomPkarr = false;

  // Saved baseline values for the inputs inside each custom-* section.
  // These mirror the values that were last persisted via `saveChanges()`.
  // Toggling a section OFF must not clobber these -- the saved baseline
  // is what `_updateUnsavedChanges()` and `saveChanges()` compare against,
  // and it is only refreshed after a successful save. When the user
  // toggles a saved custom section back ON, these baselines are restored
  // into the visible controllers so the form matches what was previously
  // saved.
  List<String> _savedRelays = <String>[];
  String _savedDnsEndpoint = '';
  String _savedDnsOriginDomain = '';
  String _savedPkarrRelay = '';

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

    final List<String>? relays =
        networkSettingsController.networkConfig.getRelays();
    if (relays != null) {
      _useCustomRelays = true;
      _relays = relays;
      _relaysInput.text = relays.join('\n');
    }

    final String? dnsEndpoint =
        networkSettingsController.networkConfig.getDnsEndpoint();
    final String? dnsOriginDomain =
        networkSettingsController.networkConfig.getDnsOriginDomain();
    if (dnsEndpoint != null || dnsOriginDomain != null) {
      _useCustomDns = true;
      _dnsEndpoint = dnsEndpoint ?? '';
      _dnsOriginDomain = dnsOriginDomain ?? '';
      _dnsEndpointInput.text = _dnsEndpoint;
      _dnsOriginDomainInput.text = _dnsOriginDomain;
    }

    final String? pkarrRelay =
        networkSettingsController.networkConfig.getPkarrRelay();
    if (pkarrRelay != null) {
      _useCustomPkarr = true;
      _pkarrRelay = pkarrRelay;
      _pkarrRelayInput.text = _pkarrRelay;
    }

    _savedUseCustomRelays = _useCustomRelays;
    _savedUseCustomDns = _useCustomDns;
    _savedUseCustomPkarr = _useCustomPkarr;
    _savedRelays = List<String>.from(_relays);
    _savedDnsEndpoint = _dnsEndpoint;
    _savedDnsOriginDomain = _dnsOriginDomain;
    _savedPkarrRelay = _pkarrRelay;
  }

  @override
  void dispose() {
    _listenPortInput.dispose();
    _bindAddressesInput.dispose();
    _relaysInput.dispose();
    _dnsEndpointInput.dispose();
    _dnsOriginDomainInput.dispose();
    _pkarrRelayInput.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    double width = widget.constraints.maxWidth < 650
        ? widget.constraints.maxWidth
        : (widget.constraints.maxWidth - 20) / 2;

    return Selector<StateController, bool>(
      selector: (context, controller) => controller.blockAudioChanges,
      builder: (BuildContext context, bool isRestartSafe, _) {
        // blockAudioChanges covers both an active call and an in-progress
        // audio test. Saving network settings requires restarting the
        // session manager, which is only safe when the audio path is idle
        // -- an audio test would otherwise be torn down mid-run, and an
        // active call would be dropped. The previous selector checked
        // isCallActive only, which let the form save during an audio test.
        return Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            if (isRestartSafe)
              Padding(
                padding: const EdgeInsets.only(bottom: 24),
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    const Icon(Icons.warning_amber_rounded,
                        color: Colors.amber),
                    const SizedBox(width: 8),
                    Expanded(
                      child: Text(
                        'Network settings cannot be changed during an active call or audio test.',
                        style: TextStyle(
                          color: Colors.amber[800],
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                    ),
                  ],
                ),
              ),
            // Critical backend error banner. The rust atomic `update`
            // collapses every poisoned-lock failure into
            // `NetworkConfigField.backendError`; that variant is not
            // tied to any user-supplied field and would be misleading
            // to paint on a specific input, so we surface it here
            // instead. Keeping it on its own row also makes the
            // severity obvious -- this is a runtime-corruption class
            // of error, not a validation message.
            if (_backendError != null)
              Padding(
                padding: const EdgeInsets.only(bottom: 24),
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    const Icon(Icons.error_outline, color: Colors.red),
                    const SizedBox(width: 8),
                    Expanded(
                      child: Text(
                        'Backend error: $_backendError',
                        style: TextStyle(
                          color: Colors.red[700],
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                    ),
                  ],
                ),
              ),
            Center(
              child: Wrap(
                spacing: 20,
                runSpacing: 20,
                children: [
                  SizedBox(
                      width: width,
                      child: TextInput(
                        labelText: 'Bind Addresses',
                        hintText: '0.0.0.0, ::, 127.0.0.1',
                        controller: _bindAddressesInput,
                        enabled: !isRestartSafe,
                        onChanged: (_) => _updateUnsavedChanges(),
                        errorText: _bindAddressesError,
                      )),
                  SizedBox(
                      width: width,
                      child: TextInput(
                        labelText: 'Listen Port',
                        controller: _listenPortInput,
                        enabled: !isRestartSafe,
                        onChanged: (_) => _updateUnsavedChanges(),
                        errorText: _listenPortError,
                      )),
                ],
              ),
            ),
            const SizedBox(height: 8),
            _buildRelaysSection(isRestartSafe),
            const SizedBox(height: 8),
            if (!kIsWeb) _buildDnsSection(width, isRestartSafe),
            if (!kIsWeb) const SizedBox(height: 8),
            _buildPkarrSection(width, isRestartSafe),
            if (unsavedChanges || _saveSucceeded) const SizedBox(height: 20),
            if (unsavedChanges)
              Button(
                text: 'Save Changes',
                onPressed: saveChanges,
                width: 100,
                disabled: _isSaving || isRestartSafe,
              )
            else if (_saveSucceeded)
              Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  const Icon(Icons.check_circle_outline, color: Colors.green),
                  const SizedBox(width: 8),
                  Text(
                    'Settings saved',
                    style: TextStyle(
                      color: Colors.green[700],
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                ],
              ),
          ],
        );
      },
    );
  }

  // The relay URLs input is a multi-line textarea, so it intentionally
  // spans the full available width rather than the half-width `width`
  // used by the single-line DNS and Pkarr inputs below.
  Widget _buildRelaysSection(bool isRestartSafe) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      mainAxisSize: MainAxisSize.min,
      children: [
        Row(
          mainAxisAlignment: MainAxisAlignment.spaceBetween,
          children: [
            const Text('Use Custom Relays',
                style: TextStyle(fontWeight: FontWeight.w600)),
            CustomSwitch(
              value: _useCustomRelays,
              disabled: isRestartSafe,
              onChanged: (value) {
                setState(() {
                  _useCustomRelays = value;
                  if (value) {
                    // Toggling a previously-saved custom section back ON
                    // restores the saved baseline into the visible
                    // controller so the form matches what was last
                    // persisted. If the section was never saved ON, the
                    // baseline is empty and the user starts from a blank
                    // form.
                    _relays = List<String>.from(_savedRelays);
                    _relaysInput.text = _savedRelays.join('\n');
                    _relaysError = null;
                  } else {
                    // Toggling OFF must not clobber the saved baseline;
                    // the dirty check and save path compare against
                    // `_savedRelays`, not the in-memory `_relays`.
                    _relays = [];
                    _relaysInput.clear();
                    _relaysError = null;
                  }
                });
                _updateUnsavedChanges();
              },
            ),
          ],
        ),
        if (_useCustomRelays) ...[
          const SizedBox(height: 10),
          SizedBox(
            width: widget.constraints.maxWidth,
            child: TextInput(
              labelText: 'Relay URLs',
              hintText: 'https://relay.example.com\nhttps://relay2.example.com',
              controller: _relaysInput,
              maxLines: 4,
              enabled: !isRestartSafe,
              onChanged: (_) => _updateUnsavedChanges(),
              errorText: _relaysError,
            ),
          ),
        ],
      ],
    );
  }

  Widget _buildDnsSection(double width, bool isRestartSafe) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      mainAxisSize: MainAxisSize.min,
      children: [
        Row(
          mainAxisAlignment: MainAxisAlignment.spaceBetween,
          children: [
            const Text('Use Custom DNS',
                style: TextStyle(fontWeight: FontWeight.w600)),
            CustomSwitch(
              value: _useCustomDns,
              disabled: isRestartSafe,
              onChanged: (value) {
                setState(() {
                  _useCustomDns = value;
                  if (value) {
                    // Toggling a previously-saved custom section back ON
                    // restores the saved baseline into the visible
                    // controllers so the form matches what was last
                    // persisted.
                    _dnsEndpoint = _savedDnsEndpoint;
                    _dnsOriginDomain = _savedDnsOriginDomain;
                    _dnsEndpointInput.text = _savedDnsEndpoint;
                    _dnsOriginDomainInput.text = _savedDnsOriginDomain;
                    _dnsEndpointError = null;
                    _dnsOriginDomainError = null;
                  } else {
                    // Toggling OFF must not clobber the saved baselines;
                    // the dirty check and save path compare against
                    // `_savedDnsEndpoint` / `_savedDnsOriginDomain`, not
                    // the in-memory fields.
                    _dnsEndpoint = '';
                    _dnsOriginDomain = '';
                    _dnsEndpointInput.clear();
                    _dnsOriginDomainInput.clear();
                    _dnsEndpointError = null;
                    _dnsOriginDomainError = null;
                  }
                });
                _updateUnsavedChanges();
              },
            ),
          ],
        ),
        if (_useCustomDns) ...[
          const SizedBox(height: 10),
          Wrap(
            spacing: 20,
            runSpacing: 20,
            children: [
              SizedBox(
                width: width,
                child: TextInput(
                  labelText: 'DNS Endpoint',
                  hintText: '127.0.0.1:5353',
                  controller: _dnsEndpointInput,
                  enabled: !isRestartSafe,
                  onChanged: (_) => _updateUnsavedChanges(),
                  errorText: _dnsEndpointError,
                ),
              ),
              SizedBox(
                width: width,
                child: TextInput(
                  labelText: 'DNS Origin Domain',
                  hintText: '_iroh.example.com.',
                  controller: _dnsOriginDomainInput,
                  enabled: !isRestartSafe,
                  onChanged: (_) => _updateUnsavedChanges(),
                  errorText: _dnsOriginDomainError,
                ),
              ),
            ],
          ),
        ],
      ],
    );
  }

  Widget _buildPkarrSection(double width, bool isRestartSafe) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      mainAxisSize: MainAxisSize.min,
      children: [
        Row(
          mainAxisAlignment: MainAxisAlignment.spaceBetween,
          children: [
            const Text('Use Custom Pkarr Relay',
                style: TextStyle(fontWeight: FontWeight.w600)),
            CustomSwitch(
              value: _useCustomPkarr,
              disabled: isRestartSafe,
              onChanged: (value) {
                setState(() {
                  _useCustomPkarr = value;
                  if (value) {
                    // Toggling a previously-saved custom section back ON
                    // restores the saved baseline into the visible
                    // controller so the form matches what was last
                    // persisted.
                    _pkarrRelay = _savedPkarrRelay;
                    _pkarrRelayInput.text = _savedPkarrRelay;
                    _pkarrRelayError = null;
                  } else {
                    // Toggling OFF must not clobber the saved baseline;
                    // the dirty check and save path compare against
                    // `_savedPkarrRelay`, not the in-memory `_pkarrRelay`.
                    _pkarrRelay = '';
                    _pkarrRelayInput.clear();
                    _pkarrRelayError = null;
                  }
                });
                _updateUnsavedChanges();
              },
            ),
          ],
        ),
        if (_useCustomPkarr) ...[
          const SizedBox(height: 10),
          SizedBox(
            width: width,
            child: TextInput(
              labelText: 'Pkarr Relay',
              hintText: 'https://pkarr.example.com',
              controller: _pkarrRelayInput,
              enabled: !isRestartSafe,
              onChanged: (_) => _updateUnsavedChanges(),
              errorText: _pkarrRelayError,
            ),
          ),
        ],
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

  /// Validates the multi-line relay URL input.
  ///
  /// Pass [requireNonEmpty] = true when the user has the "Use Custom Relays"
  /// toggle on. Enabling custom relays with zero URLs would persist
  /// `customRelaysEnabled = true` against an empty relay list, which on
  /// the rust side disables the default relay map without supplying a
  /// replacement -- effectively a misconfiguration. The caller therefore
  /// must reject the empty case in that mode.
  String? _validateRelayUrls(String text, {bool requireNonEmpty = true}) {
    final List<String> lines = text
        .split('\n')
        .map((line) => line.trim())
        .where((line) => line.isNotEmpty)
        .toList();
    if (requireNonEmpty && lines.isEmpty) {
      return 'Enter at least one relay URL';
    }
    for (final String line in lines) {
      final Uri? uri = Uri.tryParse(line);
      if (uri == null) {
        return 'Invalid URL: $line';
      }
      if (uri.scheme != 'http' && uri.scheme != 'https') {
        return 'URL must use http or https: $line';
      }
      if (uri.host.isEmpty) {
        return 'URL is missing a host: $line';
      }
    }
    return null;
  }

  String? _validateDnsEndpoint(String text) {
    final int lastColon = text.lastIndexOf(':');
    if (lastColon <= 0 || lastColon == text.length - 1) {
      return 'DNS endpoint must be host:port';
    }
    final String host = text.substring(0, lastColon);
    final String portText = text.substring(lastColon + 1);
    if (host.isEmpty) {
      return 'DNS endpoint host is required';
    }
    final int? port = int.tryParse(portText);
    if (port == null) {
      return 'DNS endpoint port must be a number';
    }
    if (port < 0 || port > 65535) {
      return 'DNS endpoint port must be between 0 and 65535';
    }
    return null;
  }

  String? _validateDnsOriginDomain(String text) {
    if (text.isEmpty) {
      return 'DNS origin domain is required';
    }
    if (!text.endsWith('.')) {
      return 'DNS origin domain must end with a dot';
    }
    return null;
  }

  String? _validatePkarrUrl(String text) {
    final Uri? uri = Uri.tryParse(text);
    if (uri == null) {
      return 'Invalid URL';
    }
    if (uri.scheme != 'http' && uri.scheme != 'https') {
      return 'URL must use http or https';
    }
    if (uri.host.isEmpty) {
      return 'URL is missing a host';
    }
    return null;
  }

  void _updateUnsavedChanges() {
    final int? listenPort = int.tryParse(_listenPortInput.text.trim());
    final List<String> bindAddresses =
        _parseBindAddresses(_bindAddressesInput.text);

    final List<String> newRelays = _useCustomRelays
        ? _relaysInput.text
            .split('\n')
            .map((line) => line.trim())
            .where((line) => line.isNotEmpty)
            .toList()
        : <String>[];

    final String newDnsEndpoint =
        _useCustomDns ? _dnsEndpointInput.text.trim() : '';
    final String newDnsOriginDomain =
        _useCustomDns ? _dnsOriginDomainInput.text.trim() : '';
    final String newPkarrRelay =
        _useCustomPkarr ? _pkarrRelayInput.text.trim() : '';

    // Compare against the saved baselines, not the in-memory fields.
    // Toggling a section OFF clears `_relays` / `_dnsEndpoint` /
    // `_dnsOriginDomain` / `_pkarrRelay`, so comparing against those
    // would report "no change" the moment a saved section is switched
    // off, which is wrong. The saved baselines are only refreshed
    // after a successful `saveChanges()`.
    setState(() {
      unsavedChanges = listenPort != _listenPort ||
          !_sameBindAddresses(bindAddresses, _bindAddresses) ||
          _useCustomRelays != _savedUseCustomRelays ||
          !_sameStringList(newRelays, _savedRelays) ||
          _useCustomDns != _savedUseCustomDns ||
          newDnsEndpoint != _savedDnsEndpoint ||
          newDnsOriginDomain != _savedDnsOriginDomain ||
          _useCustomPkarr != _savedUseCustomPkarr ||
          newPkarrRelay != _savedPkarrRelay;
    });
  }

  bool _sameStringList(List<String> first, List<String> second) {
    if (first.length != second.length) return false;
    for (int i = 0; i < first.length; i++) {
      if (first[i] != second[i]) return false;
    }
    return true;
  }

  Future<void> saveChanges() async {
    if (_isSaving) return;
    final stateController = context.read<StateController>();
    // Use the restart-safe guard rather than isCallActive: an audio test
    // also occupies the call slot on the rust side, and saving network
    // settings requires restarting the session manager, which would
    // tear down the in-progress audio test. blockAudioChanges is true
    // for both an active call and an in-progress audio test.
    if (stateController.blockAudioChanges) return;
    setState(() {
      _isSaving = true;
    });

    // Phase 1: validate all inputs without mutating the shared config.
    // Every _validate* check and cheap pre-parse runs first so the
    // setter phase below only sees values that have already been
    // accepted by the client-side checks.
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

    String? relaysError;
    List<String> relays = <String>[];
    if (_useCustomRelays) {
      relaysError = _validateRelayUrls(_relaysInput.text);
      relays = _relaysInput.text
          .split('\n')
          .map((line) => line.trim())
          .where((line) => line.isNotEmpty)
          .toList();
    }

    String? dnsEndpointError;
    String? dnsOriginDomainError;
    String dnsEndpoint = '';
    String dnsOriginDomain = '';
    if (_useCustomDns) {
      dnsEndpoint = _dnsEndpointInput.text.trim();
      dnsOriginDomain = _dnsOriginDomainInput.text.trim();
      if (dnsEndpoint.isEmpty && dnsOriginDomain.isEmpty) {
        dnsEndpointError = 'DNS endpoint is required';
        dnsOriginDomainError = 'DNS origin domain is required';
      } else {
        if (dnsEndpoint.isEmpty) {
          dnsEndpointError = 'DNS endpoint is required';
        } else {
          dnsEndpointError = _validateDnsEndpoint(dnsEndpoint);
        }
        if (dnsOriginDomain.isEmpty) {
          dnsOriginDomainError = 'DNS origin domain is required';
        } else {
          dnsOriginDomainError = _validateDnsOriginDomain(dnsOriginDomain);
        }
      }
    }

    String? pkarrRelayError;
    String pkarrRelay = '';
    if (_useCustomPkarr) {
      pkarrRelay = _pkarrRelayInput.text.trim();
      if (pkarrRelay.isEmpty) {
        pkarrRelayError = 'Pkarr relay is required';
      } else {
        pkarrRelayError = _validatePkarrUrl(pkarrRelay);
      }
    }

    final bool hasError = listenPortError != null ||
        bindAddressesError != null ||
        relaysError != null ||
        dnsEndpointError != null ||
        dnsOriginDomainError != null ||
        pkarrRelayError != null;

    if (hasError) {
      _completeFailedSave(() {
        _applyFieldErrors(
          listenPort: listenPortError,
          bindAddresses: bindAddressesError,
          relays: relaysError,
          dnsEndpoint: dnsEndpointError,
          dnsOriginDomain: dnsOriginDomainError,
          pkarrRelay: pkarrRelayError,
        );
        unsavedChanges = true;
      });
      return;
    }

    final int newListenPort = listenPort!;

    final networkSettingsController = context.read<NetworkSettingsController>();
    final telepathy = context.read<Telepathy>();
    final networkConfig = networkSettingsController.networkConfig;

    final bool listenPortChanged = newListenPort != _listenPort;
    final bool bindAddressesChanged =
        !_sameBindAddresses(bindAddresses, _bindAddresses);
    // Compare against the saved baselines, not the in-memory fields.
    // Toggling a custom section OFF clears `_relays` / `_dnsEndpoint` /
    // `_dnsOriginDomain` / `_pkarrRelay` in place, so the in-memory
    // field would falsely report "no change" the moment a saved
    // section is switched off. The saved baselines are only refreshed
    // after a successful save, so comparing against them gives a
    // correct "needs save" signal here.
    final bool relaysChanged = _useCustomRelays != _savedUseCustomRelays ||
        !_sameStringList(relays, _savedRelays);
    final bool dnsChanged = _useCustomDns != _savedUseCustomDns ||
        (_useCustomDns
            ? (dnsEndpoint != _savedDnsEndpoint ||
                dnsOriginDomain != _savedDnsOriginDomain)
            : (_savedDnsEndpoint.isNotEmpty ||
                _savedDnsOriginDomain.isNotEmpty));
    final bool pkarrChanged = _useCustomPkarr != _savedUseCustomPkarr ||
        (_useCustomPkarr
            ? pkarrRelay != _savedPkarrRelay
            : _savedPkarrRelay.isNotEmpty);

    if (!listenPortChanged &&
        !bindAddressesChanged &&
        !relaysChanged &&
        !dnsChanged &&
        !pkarrChanged) {
      // Nothing has actually changed (e.g. the user toggled a section
      // back to its prior state). Treat this as a successful no-op
      // and clear any stale error/dirty state so the form goes
      // clean -- but do so through the same helper as a failure so
      // the Save button is consistently re-enabled.
      _completeFailedSave(() {
        _clearErrors();
        unsavedChanges = false;
      });
      return;
    }

    // Phase 2: apply mutations atomically. The atomic `update` setter
    // validates every field up front and only commits if every field
    // is acceptable to the rust side. If any field is rejected, the
    // live `NetworkConfig` is left exactly as it was -- unlike the
    // per-field setters we used previously, which could leave the
    // live config partially mutated if a later setter rejected its
    // value. The rust side tags the failure with the offending field
    // via [NetworkConfigUpdateError.field]; we route the message to
    // the matching per-field error so the user can see exactly which
    // input was rejected. A [NetworkConfigField.backendError] is
    // surfaced as a critical backend error rather than on a specific
    // input -- it indicates a lock was poisoned (the rust runtime was
    // corrupted by a panic), and attributing that to any one
    // user-supplied field would be misleading.
    try {
      networkConfig.update(
        listenPort: newListenPort,
        bindAddresses: bindAddresses,
        relays: _useCustomRelays ? relays : null,
        dnsEndpoint: _useCustomDns ? dnsEndpoint : null,
        dnsOriginDomain: _useCustomDns ? dnsOriginDomain : null,
        pkarrRelay: _useCustomPkarr ? pkarrRelay : null,
      );
    } on NetworkConfigUpdateError catch (error) {
      _completeFailedSave(() {
        _clearErrors();
        _applyFieldErrors(
          listenPort: _fieldErrorText(
            error,
            NetworkConfigField.listenPort,
            _listenPortError,
          ),
          bindAddresses: _fieldErrorText(
            error,
            NetworkConfigField.bindAddresses,
            _bindAddressesError,
          ),
          relays: _fieldErrorText(
            error,
            NetworkConfigField.relays,
            _relaysError,
          ),
          dnsEndpoint: _fieldErrorText(
            error,
            NetworkConfigField.dnsEndpoint,
            _dnsEndpointError,
          ),
          dnsOriginDomain: _fieldErrorText(
            error,
            NetworkConfigField.dnsOriginDomain,
            _dnsOriginDomainError,
          ),
          pkarrRelay: _fieldErrorText(
            error,
            NetworkConfigField.pkarrRelay,
            _pkarrRelayError,
          ),
          backend: _backendErrorText(error),
        );
        unsavedChanges = true;
      });
      return;
    } on DartError catch (error) {
      // Defensive fallback: if the rust side ever surfaces a plain
      // [DartError] (e.g. from a path that bypasses the typed
      // [NetworkConfigUpdateError]), keep the previous behaviour of
      // painting the message on the bind-addresses field so the user
      // is not left without feedback.
      _completeFailedSave(() {
        _clearErrors();
        _bindAddressesError = error.message;
        unsavedChanges = true;
      });
      return;
    }

    // The atomic update has succeeded: the live NetworkConfig now
    // reflects the new values. The baseline state fields, the saved
    // baselines, the cleared errors, and `unsavedChanges = false` are
    // deferred until BOTH `saveNetworkConfig()` AND `restartManager()`
    // complete successfully below. If persistence or restart fails,
    // we must leave the form in a dirty state with a visible error so
    // the user can see the save did not stick and retry; clearing
    // `unsavedChanges` and refreshing the baselines up front would
    // silently swallow a save failure.
    try {
      await networkSettingsController.saveNetworkConfig();
      await telepathy.restartManager();
    } catch (error) {
      // Persistence or restart failed. The atomic `update` above
      // already mutated `networkConfig` in memory, so the visible
      // inputs still reflect the user's intent. We deliberately do
      // NOT roll the setter back: rolling back would require
      // recreating the rust NetworkConfig from the previous
      // (now-stale) inputs, which the public API does not expose,
      // and the user can recover by re-submitting. Surface a visible
      // error and keep the form dirty so the navigation guard does
      // not let the user leave with a half-saved configuration.
      // This is a backend-level failure rather than a per-field
      // validation, so the message goes to the dedicated backend
      // error slot rather than the bind-addresses input.
      _completeFailedSave(() {
        _clearErrors();
        _backendError = 'Failed to save or apply network settings: $error';
        unsavedChanges = true;
        _saveSucceeded = false;
      });
      return;
    } finally {
      // The persistence/restart path below intentionally does NOT
      // setState (the success path rebuilds once at the end). If we
      // reach this finally block via a thrown error, `_isSaving` was
      // already cleared inside `_completeFailedSave` above; clearing
      // it again is a no-op. If we reach here via the success path
      // (no exception), the post-save setState already cleared it
      // implicitly via the rebuilt widget tree, but we make the
      // reset explicit so the state is always well-defined regardless
      // of which branch was taken.
      _isSaving = false;
    }

    // Both persistence and manager restart succeeded. Only NOW is it
    // safe to clear `unsavedChanges` and refresh the saved baselines.
    // Doing this up front (before persistence) was the previous bug:
    // the form looked clean while the rust side was in a transient
    // state, and a save failure left the user with no way to know.
    if (mounted) {
      setState(() {
        _listenPort = newListenPort;
        _bindAddresses = bindAddresses;
        _relays = _useCustomRelays ? relays : <String>[];
        _dnsEndpoint = _useCustomDns ? dnsEndpoint : '';
        _dnsOriginDomain = _useCustomDns ? dnsOriginDomain : '';
        _pkarrRelay = _useCustomPkarr ? pkarrRelay : '';
        _savedUseCustomRelays = _useCustomRelays;
        _savedUseCustomDns = _useCustomDns;
        _savedUseCustomPkarr = _useCustomPkarr;
        _savedRelays =
            List<String>.from(_useCustomRelays ? relays : <String>[]);
        _savedDnsEndpoint = _useCustomDns ? dnsEndpoint : '';
        _savedDnsOriginDomain = _useCustomDns ? dnsOriginDomain : '';
        _savedPkarrRelay = _useCustomPkarr ? pkarrRelay : '';
        _clearErrors();
        unsavedChanges = false;
        // TODO session manager restart is slow so save succeeded takes a while to show up
        _saveSucceeded = true;
      });
    }

    // Use the canonical representation returned by the Rust setter as the
    // new baseline for both the state field and the visible input text.
    // This keeps the dirty check stable: the value the user sees, the
    // value held in state, and the value a subsequent getter would return
    // are all the same, so `_updateUnsavedChanges` will report clean.
    final String savedDnsEndpoint = networkConfig.getDnsEndpoint() ?? '';
    final String savedDnsOriginDomain =
        networkConfig.getDnsOriginDomain() ?? '';
    final List<String> savedRelays = networkConfig.getRelays() ?? <String>[];
    final String savedPkarrRelay = networkConfig.getPkarrRelay() ?? '';

    if (mounted) {
      setState(() {
        _relays = savedRelays;
        _dnsEndpoint = savedDnsEndpoint;
        _dnsOriginDomain = savedDnsOriginDomain;
        _pkarrRelay = savedPkarrRelay;
        // Mirror the canonical Rust values into the saved baselines so
        // that toggling the same custom section back on after a save
        // restores exactly what was persisted, not a stale in-memory
        // copy. `_savedUseCustom*` already reflects the user's choice
        // from the synchronous setState above; if the user kept the
        // section ON, the saved baseline must contain the canonical
        // values, otherwise it must be empty.
        _savedRelays =
            List<String>.from(_savedUseCustomRelays ? savedRelays : <String>[]);
        _savedDnsEndpoint = _savedUseCustomDns ? savedDnsEndpoint : '';
        _savedDnsOriginDomain = _savedUseCustomDns ? savedDnsOriginDomain : '';
        _savedPkarrRelay = _savedUseCustomPkarr ? savedPkarrRelay : '';
        if (_dnsEndpointInput.text != savedDnsEndpoint) {
          _dnsEndpointInput.text = savedDnsEndpoint;
        }
        if (_dnsOriginDomainInput.text != savedDnsOriginDomain) {
          _dnsOriginDomainInput.text = savedDnsOriginDomain;
        }
        final String savedRelaysText = savedRelays.join('\n');
        if (_relaysInput.text != savedRelaysText) {
          _relaysInput.text = savedRelaysText;
        }
        if (_pkarrRelayInput.text != savedPkarrRelay) {
          _pkarrRelayInput.text = savedPkarrRelay;
        }
      });
    }

    Future.delayed(const Duration(seconds: 2), () {
      if (mounted) {
        setState(() => _saveSucceeded = false);
      }
    });
  }

  /// Resets every per-field error to `null`. Intended to be used
  /// inside a `setState` callback together with a targeted error
  /// assignment on the field that just failed.
  void _clearErrors() {
    _listenPortError = null;
    _bindAddressesError = null;
    _relaysError = null;
    _dnsEndpointError = null;
    _dnsOriginDomainError = null;
    _pkarrRelayError = null;
    _backendError = null;
  }

  /// Sets every per-field error to the supplied value (which may be
  /// `null`). Lets the validation-failed branch reset the whole
  /// error surface with a single call. [backend] is the critical
  /// backend error path used by `NetworkConfigField.backendError`; it
  /// is intentionally separate from the per-input slots so a poison
  /// failure is not silently attributed to whichever input the user
  /// happened to focus on.
  void _applyFieldErrors({
    String? listenPort,
    String? bindAddresses,
    String? relays,
    String? dnsEndpoint,
    String? dnsOriginDomain,
    String? pkarrRelay,
    String? backend,
  }) {
    _listenPortError = listenPort;
    _bindAddressesError = bindAddresses;
    _relaysError = relays;
    _dnsEndpointError = dnsEndpoint;
    _dnsOriginDomainError = dnsOriginDomain;
    _pkarrRelayError = pkarrRelay;
    _backendError = backend;
  }

  /// Map a [NetworkConfigUpdateError] to a per-field error message.
  ///
  /// If the failure is for the field corresponding to [field], return
  /// the error's message; otherwise leave the existing [existing]
  /// value untouched. This lets the rust-side `field` tag drive which
  /// slot gets the message while every other slot keeps whatever it
  /// had (typically `null`, since callers pair this with
  /// `_clearErrors()`).
  String? _fieldErrorText(
    NetworkConfigUpdateError error,
    NetworkConfigField field,
    String? existing,
  ) {
    if (error.field == field) return error.message;
    return existing;
  }

  /// If the rust-side error is a critical backend failure
  /// ([NetworkConfigField.backendError]), surface the message in the
  /// dedicated backend-error slot. Otherwise return `null` so the
  /// existing value (typically `null`) is preserved.
  String? _backendErrorText(NetworkConfigUpdateError error) {
    if (error.field == NetworkConfigField.backendError) {
      return error.message;
    }
    return null;
  }

  /// Completes a save attempt that did NOT result in a successful
  /// persistence/restart, by applying [apply] inside a single
  /// `setState` call and resetting `_isSaving` so the Save button is
  /// re-enabled for retry. The previous version of this code reset
  /// `_isSaving` separately on each early-return path, which was
  /// easy to forget; one missed branch left the Save button stuck in
  /// its disabled state forever. Funneling every failure through this
  /// helper makes the reset unconditional and the widget rebuild
  /// consistent across all error/no-op branches.
  void _completeFailedSave(VoidCallback apply) {
    if (!mounted) {
      // If the widget was disposed mid-save there is no rebuild to
      // schedule, but we still need to clear the in-flight flag so
      // the state is consistent if it is ever read again.
      _isSaving = false;
      return;
    }
    setState(() {
      _isSaving = false;
      apply();
    });
  }
}
