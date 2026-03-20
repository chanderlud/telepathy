import 'package:collection/collection.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart' hide TextInput;
import 'package:flutter_svg/flutter_svg.dart';
import 'package:provider/provider.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/core/utils/index.dart';
import 'package:telepathy/core/utils/room_format_utils.dart';
import 'package:telepathy/src/rust/error.dart';
import 'package:telepathy/src/rust/telepathy.dart';
import 'package:telepathy/widgets/common/index.dart';
import 'package:telepathy/src/rust/flutter.dart';

/// A widget which allows the user to add a contact.
class ContactForm extends StatefulWidget {
  const ContactForm({super.key});

  @override
  State<ContactForm> createState() => ContactFormState();
}

/// The state for ContactForm.
class ContactFormState extends State<ContactForm> {
  final TextEditingController _nicknameInput = TextEditingController();
  final TextEditingController _peerIdInput = TextEditingController();
  final List<String> _peerIds = [];
  final FocusNode _nicknameFocusNode = FocusNode();
  String? selectedPeer;
  bool? addContact;
  String? _contactNicknameError;
  String? _contactPeerIdError;
  String? _roomNicknameError;

  @override
  void dispose() {
    _nicknameFocusNode.dispose();
    _nicknameInput.dispose();
    _peerIdInput.dispose();
    super.dispose();
  }

  Widget? _errorText(BuildContext context, String? message) {
    if (message == null) return null;
    return Text(
      message,
      style: TextStyle(
        color: Theme.of(context).colorScheme.error,
        fontSize: 12,
      ),
    );
  }

  String _peerChipLabel(String peerId, ProfilesController profilesController) {
    final contact = profilesController.contacts.values
        .firstWhereOrNull((c) => c.peerId() == peerId);
    if (contact != null) return contact.nickname();
    if (peerId == profilesController.peerId) return 'You';
    if (peerId.length > 14) {
      return '${peerId.substring(0, 14)}…';
    }
    return peerId;
  }

  void _setAddMode(bool contact) {
    setState(() {
      addContact = contact;
      if (contact) {
        _roomNicknameError = null;
      } else {
        _contactNicknameError = null;
        _contactPeerIdError = null;
      }
    });
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _nicknameFocusNode.requestFocus();
    });
  }

  Widget _segmentChip(
    BuildContext context, {
    required String label,
    required bool contactMode,
  }) {
    final scheme = Theme.of(context).colorScheme;
    final selected = addContact != null && addContact == contactMode;
    return Expanded(
      child: Material(
        color: Colors.transparent,
        child: InkWell(
          onTap: () => _setAddMode(contactMode),
          borderRadius: BorderRadius.circular(8),
          child: Container(
            padding: const EdgeInsets.symmetric(vertical: 12, horizontal: 8),
            decoration: BoxDecoration(
              color:
                  selected ? scheme.primary : scheme.tertiaryContainer,
              borderRadius: BorderRadius.circular(8),
            ),
            alignment: Alignment.center,
            child: Text(
              label,
              style: TextStyle(
                fontWeight: FontWeight.w600,
                color: selected
                    ? scheme.onPrimary
                    : scheme.onTertiaryContainer,
              ),
            ),
          ),
        ),
      ),
    );
  }

  /// Segmented Contact | Room control. [showBack] adds a back control that
  /// clears the mode (returns to chooser).
  Widget _segmentedControl(BuildContext context, {required bool showBack}) {
    final segments = Row(
      children: [
        _segmentChip(context, label: 'Contact', contactMode: true),
        const SizedBox(width: 8),
        _segmentChip(context, label: 'Room', contactMode: false),
      ],
    );
    if (!showBack) return segments;
    return Row(
      crossAxisAlignment: CrossAxisAlignment.center,
      children: [
        IconButton(
          style: IconButton.styleFrom(
            minimumSize: const Size(48, 48),
          ),
          onPressed: () {
            setState(() {
              addContact = null;
              _contactNicknameError = null;
              _contactPeerIdError = null;
              _roomNicknameError = null;
            });
          },
          icon: SvgPicture.asset('assets/icons/Back.svg',
              semanticsLabel: 'Back'),
        ),
        Expanded(child: segments),
      ],
    );
  }

  @override
  Widget build(BuildContext context) {
    final telepathy = context.read<Telepathy>();
    final profilesController = context.read<ProfilesController>();

    if (addContact == null) {
      return Container(
        padding: const EdgeInsets.symmetric(vertical: 15.0, horizontal: 20.0),
        decoration: BoxDecoration(
          color: Theme.of(context).colorScheme.secondaryContainer,
          borderRadius: BorderRadius.circular(10.0),
        ),
        child: _segmentedControl(context, showBack: false),
      );
    } else if (addContact == true) {
      final scheme = Theme.of(context).colorScheme;
      return Container(
        padding: const EdgeInsets.symmetric(vertical: 15.0, horizontal: 20.0),
        constraints: const BoxConstraints(maxWidth: 360),
        decoration: BoxDecoration(
          color: scheme.secondaryContainer,
          borderRadius: BorderRadius.circular(10.0),
        ),
        child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              _segmentedControl(context, showBack: true),
              const SizedBox(height: 16),
              TextInput(
                controller: _nicknameInput,
                labelText: 'Nickname',
                focusNode: _nicknameFocusNode,
                error: _errorText(context, _contactNicknameError),
                onChanged: (_) {
                  if (_contactNicknameError != null) {
                    setState(() => _contactNicknameError = null);
                  }
                },
              ),
              Divider(
                height: 28,
                color: scheme.onSecondaryContainer.withValues(alpha: 0.2),
              ),
              TextInput(
                controller: _peerIdInput,
                labelText: 'Peer ID',
                hintText: 'string encoded peer ID',
                obscureText: true,
                error: _errorText(context, _contactPeerIdError),
                onChanged: (_) {
                  if (_contactPeerIdError != null) {
                    setState(() => _contactPeerIdError = null);
                  }
                },
              ),
              const SizedBox(height: 26),
              Center(
                child: Button(
                  text: 'Add Contact',
                  onPressed: () async {
                    String nickname = _nicknameInput.text.trim();
                    String peerId = _peerIdInput.text.trim();

                    setState(() {
                      _contactNicknameError =
                          nickname.isEmpty ? 'Nickname is required' : null;
                      _contactPeerIdError =
                          peerId.isEmpty ? 'Peer ID is required' : null;
                    });
                    if (nickname.isEmpty || peerId.isEmpty) {
                      return;
                    } else if (profilesController.contacts.values
                        .any((c) => c.peerId() == peerId)) {
                      showErrorDialog(context, 'Failed to add contact',
                          'Contact for peer ID already exists');
                      return;
                    } else if (profilesController.peerId == peerId) {
                      showErrorDialog(context, 'Failed to add contact',
                          'Cannot add self as a contact');
                      return;
                    }

                    try {
                      Contact contact =
                          profilesController.addContact(nickname, peerId);

                      telepathy.startSession(contact: contact);

                      _nicknameInput.clear();
                      _peerIdInput.clear();
                      if (!context.mounted) return;
                      Navigator.pop(context);
                    } on DartError catch (_) {
                      showErrorDialog(
                          context, 'Failed to add contact', 'Invalid peer ID');
                    }
                  },
                ),
              ),
            ],
          ),
      );
    } else {
      var contacts = profilesController.contacts.values
          .where((c) => !_peerIds.contains(c.peerId()));

      final scheme = Theme.of(context).colorScheme;

      return Container(
        padding: const EdgeInsets.symmetric(vertical: 15.0, horizontal: 20.0),
        decoration: BoxDecoration(
          color: scheme.secondaryContainer,
          borderRadius: BorderRadius.circular(10.0),
        ),
        constraints: const BoxConstraints(maxWidth: 400),
        width: double.infinity,
        child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                crossAxisAlignment: CrossAxisAlignment.center,
                children: [
                  Expanded(
                    child: _segmentedControl(context, showBack: true),
                  ),
                  Tooltip(
                    message: 'Paste room details from clipboard',
                    child: IconButton(
                      style: IconButton.styleFrom(
                        minimumSize: const Size(48, 48),
                      ),
                      onPressed: () async {
                        final data =
                            await Clipboard.getData(Clipboard.kTextPlain);
                        final text = data?.text?.trim();

                        if (text == null || text.isEmpty) {
                          if (!context.mounted) return;
                          showErrorDialog(context, 'Failed to paste room details',
                              'Clipboard does not contain any text');
                          return;
                        }

                        final parsed = parseRoomDetails(text);
                        if (parsed == null) {
                          if (!context.mounted) return;
                          showErrorDialog(
                              context,
                              'Failed to paste room details',
                              'Clipboard text is not a valid room details format');
                          return;
                        }

                        final nickname = parsed.nickname.trim();
                        final peerIds = parsed.peerIds
                            .map((p) => p.trim())
                            .where((p) => p.isNotEmpty)
                            .toSet()
                            .toList();

                        if (nickname.isEmpty) {
                          if (!context.mounted) return;
                          showErrorDialog(context, 'Failed to paste room details',
                              'Room nickname cannot be empty');
                          return;
                        }

                        if (peerIds.isEmpty) {
                          if (!context.mounted) return;
                          showErrorDialog(context, 'Failed to paste room details',
                              'Room peer IDs cannot be empty');
                          return;
                        }

                        final invalid = peerIds.firstWhereOrNull(
                            (p) => !validatePeerId(peerId: p));
                        if (invalid != null) {
                          if (!context.mounted) return;
                          showErrorDialog(context, 'Failed to paste room details',
                              'Invalid peer ID: $invalid');
                          return;
                        }

                        _nicknameInput.text = nickname;
                        _peerIdInput.clear();
                        if (!context.mounted) return;
                        setState(() {
                          _peerIds
                            ..clear()
                            ..addAll(peerIds);
                        });
                      },
                      icon: SvgPicture.asset('assets/icons/Copy.svg'),
                    ),
                  ),
                ],
              ),
              const SizedBox(height: 12),
              TextInput(
                controller: _nicknameInput,
                labelText: 'Nickname',
                focusNode: _nicknameFocusNode,
                error: _errorText(context, _roomNicknameError),
                onChanged: (_) {
                  if (_roomNicknameError != null) {
                    setState(() => _roomNicknameError = null);
                  }
                },
              ),
              const SizedBox(height: 15),
              Row(
                children: [
                  Expanded(
                    child: TextInput(
                      controller: _peerIdInput,
                      labelText: 'Peer ID',
                      hintText: 'string encoded peer ID',
                      obscureText: true,
                    ),
                  ),
                  const SizedBox(width: 8),
                  IconButton(
                    style: IconButton.styleFrom(
                      minimumSize: const Size(48, 48),
                    ),
                    icon: SvgPicture.asset('assets/icons/Plus.svg'),
                    onPressed: () {
                      if (_peerIds.contains(_peerIdInput.text)) {
                        return;
                      } else if (validatePeerId(peerId: _peerIdInput.text)) {
                        setState(() {
                          _peerIds.add(_peerIdInput.text);
                          _peerIdInput.clear();
                        });
                      } else {
                        showErrorDialog(context, 'Failed to add Peer ID',
                            'The provided Peer ID is invalid');
                      }
                    },
                  ),
                ],
              ),
              if (_peerIds.isNotEmpty) ...[
                const SizedBox(height: 12),
                Wrap(
                  spacing: 8,
                  runSpacing: 8,
                  children: _peerIds.map((peerId) {
                    return InputChip(
                      label: Text(_peerChipLabel(peerId, profilesController)),
                      onDeleted: () {
                        setState(() {
                          _peerIds.remove(peerId);
                        });
                      },
                      deleteIcon: SvgPicture.asset('assets/icons/Trash.svg',
                          width: 18,
                          height: 18,
                          semanticsLabel: 'Remove peer'),
                    );
                  }).toList(),
                ),
              ],
              if (contacts.isNotEmpty) const SizedBox(height: 15),
              if (contacts.isNotEmpty)
                Row(
                  crossAxisAlignment: CrossAxisAlignment.center,
                  children: [
                    Expanded(
                      child: LayoutBuilder(
                        builder: (context, constraints) {
                          return DropDown(
                            items: contacts
                                .map((c) => (c.peerId(), c.nickname()))
                                .toList(),
                            initialSelection:
                                contacts.elementAtOrNull(0)?.peerId(),
                            onSelected: (selected) => {
                              setState(() {
                                selectedPeer = selected;
                              })
                            },
                            label: 'Contact',
                            width: constraints.maxWidth,
                          );
                        },
                      ),
                    ),
                    const SizedBox(width: 8),
                    IconButton(
                      style: IconButton.styleFrom(
                        minimumSize: const Size(48, 48),
                      ),
                      icon: SvgPicture.asset('assets/icons/Plus.svg'),
                      onPressed: () {
                        String? peerId = selectedPeer ??
                            contacts.elementAtOrNull(0)?.peerId();
                        if (peerId != null && !_peerIds.contains(peerId)) {
                          setState(() {
                            _peerIds.add(peerId);
                          });
                        }
                      },
                    ),
                  ],
                ),
              const SizedBox(height: 20),
              Row(
                mainAxisAlignment: MainAxisAlignment.center,
                children: [
                  Container(
                    padding: const EdgeInsets.symmetric(
                        horizontal: 12, vertical: 6),
                    decoration: BoxDecoration(
                      color: scheme.primary.withValues(alpha: 0.2),
                      borderRadius: BorderRadius.circular(20),
                    ),
                    child: Text(
                      '${_peerIds.length} peer${_peerIds.length == 1 ? '' : 's'}',
                      style: TextStyle(
                        fontWeight: FontWeight.w600,
                        color: scheme.onSecondaryContainer,
                        fontSize: 13,
                      ),
                    ),
                  ),
                ],
              ),
              const SizedBox(height: 16),
              Row(
                mainAxisAlignment: MainAxisAlignment.center,
                children: [
                  Button(
                    text: 'Cancel',
                    onPressed: () {
                      Navigator.pop(context);
                    },
                  ),
                  const SizedBox(width: 16),
                  Button(
                    text: 'Add room',
                    onPressed: () async {
                      String nickname = _nicknameInput.text.trim();

                      setState(() {
                        _roomNicknameError =
                            nickname.isEmpty ? 'Nickname is required' : null;
                      });
                      if (nickname.isEmpty) {
                        return;
                      } else if (_peerIds.isEmpty) {
                        showErrorDialog(context, 'Failed to add room',
                            'Peer IDs cannot be empty');
                        return;
                      }

                      try {
                        if (!_peerIds.contains(profilesController.peerId)) {
                          _peerIds.add(profilesController.peerId);
                        }

                        if (profilesController.rooms.keys
                            .contains(roomHash(peers: _peerIds))) {
                          showErrorDialog(context, 'Failed to add room',
                              'It appears this room already exists');
                          return;
                        }

                        profilesController.addRoom(nickname, List.from(_peerIds));
                        _nicknameInput.clear();
                        setState(() {
                          _peerIds.clear();
                        });
                        if (!context.mounted) return;
                        Navigator.pop(context);
                      } on DartError catch (error) {
                        showErrorDialog(context, 'Failed to add room',
                            'Invalid peer ID: ${error.message}');
                      }
                    },
                  ),
                ],
              ),
            ],
          ),
      );
    }
  }
}
