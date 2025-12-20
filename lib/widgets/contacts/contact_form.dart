import 'package:collection/collection.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart' hide TextInput;
import 'package:flutter_svg/flutter_svg.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/core/utils/index.dart';
import 'package:telepathy/core/utils/room_format_utils.dart';
import 'package:telepathy/src/rust/error.dart';
import 'package:telepathy/src/rust/telepathy.dart';
import 'package:telepathy/widgets/common/index.dart';
import 'package:telepathy/src/rust/flutter.dart';

/// A widget which allows the user to add a contact.
class ContactForm extends StatefulWidget {
  final Telepathy telepathy;
  final ProfilesController profilesController;

  const ContactForm(
      {super.key, required this.telepathy, required this.profilesController});

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

  @override
  void dispose() {
    _nicknameFocusNode.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    if (addContact == null) {
      return Container(
        padding: const EdgeInsets.symmetric(vertical: 15.0, horizontal: 20.0),
        decoration: BoxDecoration(
          color: Theme.of(context).colorScheme.secondaryContainer,
          borderRadius: BorderRadius.circular(10.0),
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Button(
              text: 'Add Contact',
              onPressed: () async {
                setState(() {
                  addContact = true;
                });
                WidgetsBinding.instance.addPostFrameCallback((_) {
                  _nicknameFocusNode.requestFocus();
                });
              },
            ),
            Button(
              text: 'Add Room',
              onPressed: () async {
                setState(() {
                  addContact = false;
                });
                WidgetsBinding.instance.addPostFrameCallback((_) {
                  _nicknameFocusNode.requestFocus();
                });
              },
            )
          ],
        ),
      );
    } else if (addContact == true) {
      return Container(
        padding: const EdgeInsets.symmetric(vertical: 15.0, horizontal: 20.0),
        constraints: const BoxConstraints(maxWidth: 250),
        decoration: BoxDecoration(
          color: Theme.of(context).colorScheme.secondaryContainer,
          borderRadius: BorderRadius.circular(10.0),
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const Text('Add Contact', style: TextStyle(fontSize: 20)),
            const SizedBox(height: 21),
            TextInput(
              controller: _nicknameInput,
              labelText: 'Nickname',
              focusNode: _nicknameFocusNode,
            ),
            const SizedBox(height: 15),
            TextInput(
                controller: _peerIdInput,
                labelText: 'Peer ID',
                hintText: 'string encoded peer ID',
                obscureText: true),
            const SizedBox(height: 26),
            Center(
              child: Button(
                text: 'Add Contact',
                onPressed: () async {
                  String nickname = _nicknameInput.text;
                  String peerId = _peerIdInput.text;

                  if (nickname.isEmpty || peerId.isEmpty) {
                    showErrorDialog(context, 'Failed to add contact',
                        'Nickname and peer id cannot be empty');
                    return;
                  } else if (widget.profilesController.contacts.values
                      .any((c) => c.peerId() == peerId)) {
                    showErrorDialog(context, 'Failed to add contact',
                        'Contact for peer ID already exists');
                    return;
                  } else if (widget.profilesController.peerId == peerId) {
                    showErrorDialog(context, 'Failed to add contact',
                        'Cannot add self as a contact');
                    return;
                  }

                  try {
                    Contact contact =
                        widget.profilesController.addContact(nickname, peerId);

                    widget.telepathy.startSession(contact: contact);

                    _nicknameInput.clear();
                    _peerIdInput.clear();
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
      var contacts = widget.profilesController.contacts.values
          .where((c) => !_peerIds.contains(c.peerId()));

      return Container(
        padding: const EdgeInsets.symmetric(vertical: 15.0, horizontal: 20.0),
        decoration: BoxDecoration(
          color: Theme.of(context).colorScheme.secondaryContainer,
          borderRadius: BorderRadius.circular(10.0),
        ),
        constraints: const BoxConstraints(maxWidth: 300),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              crossAxisAlignment: CrossAxisAlignment.center,
              children: [
                const Padding(
                  padding: EdgeInsetsGeometry.directional(bottom: 7),
                  child: Text('Add Room', style: TextStyle(fontSize: 20)),
                ),
                const SizedBox(width: 12),
                IconButton(
                  onPressed: () async {
                    final data = await Clipboard.getData(Clipboard.kTextPlain);
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
                      showErrorDialog(context, 'Failed to paste room details',
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

                    final invalid = peerIds
                        .firstWhereOrNull((p) => !validatePeerId(peerId: p));
                    if (invalid != null) {
                      if (!context.mounted) return;
                      showErrorDialog(context, 'Failed to paste room details',
                          'Invalid peer ID: $invalid');
                      return;
                    }

                    _nicknameInput.text = nickname;
                    _peerIdInput.clear();
                    setState(() {
                      _peerIds
                        ..clear()
                        ..addAll(peerIds);
                    });
                  },
                  icon: SvgPicture.asset('assets/icons/Copy.svg'),
                  constraints:
                      const BoxConstraints(maxWidth: 32, maxHeight: 32),
                  padding: const EdgeInsetsGeometry.directional(
                      start: 7, top: 7, end: 7, bottom: 7),
                )
              ],
            ),
            const SizedBox(height: 15),
            TextInput(
              controller: _nicknameInput,
              labelText: 'Nickname',
              focusNode: _nicknameFocusNode,
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
                const SizedBox(width: 16),
                IconButton(
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
            if (contacts.isNotEmpty) const SizedBox(height: 15),
            if (contacts.isNotEmpty)
              Row(
                children: [
                  Expanded(
                    child: DropDown(
                      items: contacts
                          .map((c) => (c.peerId(), c.nickname()))
                          .toList(),
                      initialSelection: contacts.elementAtOrNull(0)?.peerId(),
                      onSelected: (selected) => {
                        setState(() {
                          selectedPeer = selected;
                        })
                      },
                      label: 'Contact',
                      width: 250,
                    ),
                  ),
                  const SizedBox(width: 16),
                  IconButton(
                    icon: SvgPicture.asset('assets/icons/Plus.svg'),
                    onPressed: () {
                      String? peerId =
                          selectedPeer ?? contacts.elementAtOrNull(0)?.peerId();
                      if (peerId != null && !_peerIds.contains(peerId)) {
                        setState(() {
                          _peerIds.add(peerId);
                        });
                      }
                    },
                  ),
                ],
              ),
            const SizedBox(height: 26),
            Center(
                child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Text('Peers: ${_peerIds.length}'),
                const SizedBox(width: 24),
                Button(
                  text: 'Add room',
                  onPressed: () async {
                    String nickname = _nicknameInput.text;

                    try {
                      if (nickname.isEmpty) {
                        showErrorDialog(context, 'Failed to add room',
                            'Nickname cannot be empty');
                        return;
                      } else if (_peerIds.isEmpty) {
                        showErrorDialog(context, 'Failed to add room',
                            'Peer IDs cannot be empty');
                        return;
                      }

                      // the room must always contain the current profile's peer id
                      if (!_peerIds
                          .contains(widget.profilesController.peerId)) {
                        _peerIds.add(widget.profilesController.peerId);
                      }

                      if (widget.profilesController.rooms.keys
                          .contains(roomHash(peers: _peerIds))) {
                        showErrorDialog(context, 'Failed to add room',
                            'It appears this room already exists');
                        return;
                      }

                      widget.profilesController.addRoom(nickname, _peerIds);
                      _nicknameInput.clear();
                      setState(() {
                        _peerIds.clear();
                      });
                      Navigator.pop(context);
                    } on DartError catch (error) {
                      showErrorDialog(context, 'Failed to add room',
                          'Invalid peer ID: ${error.message}');
                    }
                  },
                )
              ],
            )),
          ],
        ),
      );
    }
  }
}
