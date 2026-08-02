import 'package:collection/collection.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart' hide TextInput;
import 'package:flutter_svg/flutter_svg.dart';
import 'package:provider/provider.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/core/utils/room_format_utils.dart';
import 'package:telepathy/core/rust/flutter.dart';
import 'package:telepathy/widgets/common/index.dart';
import 'package:telepathy/core/rust/types.dart';

/// Opens the add contact / add room flow.
Future<void> showAddEntryDialog(BuildContext context) {
  return showDialog(
    context: context,
    builder: (BuildContext context) => const AddEntryDialog(),
  );
}

enum _AddEntryView { chooser, contact, room }

/// A dialog which guides the user through adding a contact or a room.
///
/// The flow starts at a chooser with one card per entry type, then swaps to
/// the matching form. Both forms validate inline (no modal error popups) and
/// offer a way back to the chooser.
class AddEntryDialog extends StatefulWidget {
  const AddEntryDialog({super.key});

  @override
  State<AddEntryDialog> createState() => _AddEntryDialogState();
}

class _AddEntryDialogState extends State<AddEntryDialog> {
  _AddEntryView _view = _AddEntryView.chooser;

  static const double _dialogWidth = 380;

  @override
  Widget build(BuildContext context) {
    final Widget body = switch (_view) {
      _AddEntryView.chooser => _Chooser(
          onContact: () => setState(() => _view = _AddEntryView.contact),
          onRoom: () => setState(() => _view = _AddEntryView.room),
        ),
      _AddEntryView.contact => const _AddContactForm(),
      _AddEntryView.room => const _AddRoomForm(),
    };

    return Dialog(
      backgroundColor: Theme.of(context).colorScheme.secondaryContainer,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(10)),
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: _dialogWidth),
        child: Padding(
          padding: const EdgeInsets.all(20),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              _DialogHeader(
                title: switch (_view) {
                  _AddEntryView.chooser => 'Add New',
                  _AddEntryView.contact => 'Add Contact',
                  _AddEntryView.room => 'Add Room',
                },
                onBack: _view == _AddEntryView.chooser
                    ? null
                    : () => setState(() => _view = _AddEntryView.chooser),
              ),
              const SizedBox(height: 16),
              Flexible(child: SingleChildScrollView(child: body)),
            ],
          ),
        ),
      ),
    );
  }
}

class _DialogHeader extends StatelessWidget {
  final String title;
  final VoidCallback? onBack;

  const _DialogHeader({required this.title, this.onBack});

  @override
  Widget build(BuildContext context) {
    return Row(
      children: [
        if (onBack != null) ...[
          SizedBox.square(
            dimension: 32,
            child: IconButton(
              onPressed: onBack,
              padding: EdgeInsets.zero,
              constraints: const BoxConstraints.tightFor(width: 32, height: 32),
              icon: SvgPicture.asset('assets/icons/Back.svg',
                  width: 22, semanticsLabel: 'Back'),
              tooltip: 'Back',
            ),
          ),
          const SizedBox(width: 8),
        ],
        Text(title, style: const TextStyle(fontSize: 20)),
      ],
    );
  }
}

/// The first step: pick between adding a contact or a room.
class _Chooser extends StatelessWidget {
  final VoidCallback onContact;
  final VoidCallback onRoom;

  const _Chooser({required this.onContact, required this.onRoom});

  @override
  Widget build(BuildContext context) {
    return Row(
      children: [
        Expanded(
          child: _OptionCard(
            icon: 'assets/icons/Profile.svg',
            title: 'Contact',
            description: 'Call one person directly',
            onTap: onContact,
          ),
        ),
        const SizedBox(width: 12),
        Expanded(
          child: _OptionCard(
            icon: 'assets/icons/Group.svg',
            title: 'Room',
            description: 'Group call with several peers',
            onTap: onRoom,
          ),
        ),
      ],
    );
  }
}

class _OptionCard extends StatefulWidget {
  final String icon;
  final String title;
  final String description;
  final VoidCallback onTap;

  const _OptionCard({
    required this.icon,
    required this.title,
    required this.description,
    required this.onTap,
  });

  @override
  State<_OptionCard> createState() => _OptionCardState();
}

class _OptionCardState extends State<_OptionCard> {
  bool _hovered = false;

  @override
  Widget build(BuildContext context) {
    final colorScheme = Theme.of(context).colorScheme;

    return InkWell(
      mouseCursor: SystemMouseCursors.click,
      onTap: widget.onTap,
      onHover: (hovered) => setState(() => _hovered = hovered),
      borderRadius: BorderRadius.circular(10),
      hoverColor: Colors.transparent,
      child: AnimatedContainer(
        duration: const Duration(milliseconds: 120),
        height: 164,
        padding: const EdgeInsets.symmetric(vertical: 20, horizontal: 12),
        decoration: BoxDecoration(
          color: colorScheme.tertiaryContainer,
          borderRadius: BorderRadius.circular(10),
          border: Border.all(
            color: _hovered ? colorScheme.primary : Colors.transparent,
            width: 1.5,
          ),
        ),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            CircleAvatar(
              maxRadius: 22,
              child: SvgPicture.asset(widget.icon, width: 26),
            ),
            const SizedBox(height: 12),
            Text(widget.title, style: const TextStyle(fontSize: 16)),
            const SizedBox(height: 4),
            Text(
              widget.description,
              textAlign: TextAlign.center,
              style: TextStyle(fontSize: 12, color: Colors.grey.shade400),
            ),
          ],
        ),
      ),
    );
  }
}

/// Inline (non-modal) error line used by both forms.
class _ErrorLine extends StatelessWidget {
  final String message;

  const _ErrorLine(this.message);

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(top: 10),
      child: Text(
        message,
        style: TextStyle(
          fontSize: 13,
          color: Theme.of(context).colorScheme.error,
        ),
      ),
    );
  }
}

/// The add-contact form: nickname plus a single peer id.
class _AddContactForm extends StatefulWidget {
  const _AddContactForm();

  @override
  State<_AddContactForm> createState() => _AddContactFormState();
}

class _AddContactFormState extends State<_AddContactForm> {
  final TextEditingController _nicknameInput = TextEditingController();
  final TextEditingController _peerIdInput = TextEditingController();
  final FocusNode _nicknameFocusNode = FocusNode();
  String? _error;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _nicknameFocusNode.requestFocus();
    });
  }

  @override
  void dispose() {
    _nicknameInput.dispose();
    _peerIdInput.dispose();
    _nicknameFocusNode.dispose();
    super.dispose();
  }

  Future<void> _pastePeerId() async {
    final String? text = (await Clipboard.getData(Clipboard.kTextPlain))?.text;
    if (text == null || text.trim().isEmpty) return;
    _peerIdInput.text = text.trim();
    setState(() => _error = null);
  }

  void _submit(ProfilesController profilesController, Telepathy telepathy) {
    final String nickname = _nicknameInput.text.trim();
    final String peerId = _peerIdInput.text.trim();

    if (profilesController.contacts.values.any((c) => c.peerId() == peerId)) {
      setState(() => _error = 'A contact for this peer ID already exists');
      return;
    } else if (profilesController.peerId == peerId) {
      setState(() => _error = 'You cannot add yourself as a contact');
      return;
    }

    try {
      final Contact contact = profilesController.addContact(nickname, peerId);
      telepathy.startSession(contact: contact);
      Navigator.pop(context);
    } on DartError catch (_) {
      setState(() => _error = 'Invalid peer ID');
    }
  }

  @override
  Widget build(BuildContext context) {
    final telepathy = context.read<Telepathy>();
    final profilesController = context.read<ProfilesController>();

    final bool canSubmit = _nicknameInput.text.trim().isNotEmpty &&
        _peerIdInput.text.trim().isNotEmpty;

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        TextInput(
          controller: _nicknameInput,
          labelText: 'Nickname',
          focusNode: _nicknameFocusNode,
          onChanged: (_) => setState(() => _error = null),
          onSubmitted: (_) {
            if (canSubmit) _submit(profilesController, telepathy);
          },
        ),
        const SizedBox(height: 14),
        Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Expanded(
              child: TextInput(
                controller: _peerIdInput,
                labelText: 'Peer ID',
                hintText: 'string encoded peer ID',
                onChanged: (_) => setState(() => _error = null),
                onSubmitted: (_) {
                  if (canSubmit) _submit(profilesController, telepathy);
                },
              ),
            ),
            const SizedBox(width: 8),
            IconButton(
              onPressed: _pastePeerId,
              icon: SvgPicture.asset('assets/icons/Copy.svg',
                  width: 24, semanticsLabel: 'Paste peer ID'),
              tooltip: 'Paste from clipboard',
            ),
          ],
        ),
        if (_error != null) _ErrorLine(_error!),
        const SizedBox(height: 18),
        Center(
          child: Button(
            text: 'Add Contact',
            disabled: !canSubmit,
            onPressed: () => _submit(profilesController, telepathy),
          ),
        ),
      ],
    );
  }
}

/// The add-room form: nickname plus a list of member peer ids, built from
/// typed ids, known contacts, or pasted room details.
class _AddRoomForm extends StatefulWidget {
  const _AddRoomForm();

  @override
  State<_AddRoomForm> createState() => _AddRoomFormState();
}

class _AddRoomFormState extends State<_AddRoomForm> {
  final TextEditingController _nicknameInput = TextEditingController();
  final TextEditingController _peerIdInput = TextEditingController();
  final FocusNode _nicknameFocusNode = FocusNode();
  final List<String> _peerIds = [];
  String? selectedPeer;
  String? _error;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _nicknameFocusNode.requestFocus();
    });
  }

  @override
  void dispose() {
    _nicknameInput.dispose();
    _peerIdInput.dispose();
    _nicknameFocusNode.dispose();
    super.dispose();
  }

  void _addPeerId(ProfilesController profilesController, String peerId) {
    final String trimmed = peerId.trim();

    if (trimmed.isEmpty) {
      return;
    } else if (_peerIds.contains(trimmed)) {
      setState(() => _error = 'This peer ID is already in the room');
      return;
    } else if (trimmed == profilesController.peerId) {
      setState(() => _error = 'You are always a member of your own rooms');
      return;
    } else if (!profilesController.isValidPeerId(trimmed)) {
      setState(() => _error = 'The provided peer ID is invalid');
      return;
    }

    setState(() {
      _error = null;
      _peerIds.add(trimmed);
      _peerIdInput.clear();
    });
  }

  Future<void> _pasteRoomDetails(ProfilesController profilesController) async {
    final String? text =
        (await Clipboard.getData(Clipboard.kTextPlain))?.text?.trim();

    if (text == null || text.isEmpty) {
      setState(() => _error = 'Clipboard does not contain any text');
      return;
    }

    final parsed = parseRoomDetails(text);
    if (parsed == null) {
      setState(() => _error = 'Clipboard text is not valid room details');
      return;
    }

    final invalid = parsed.peerIds
        .firstWhereOrNull((p) => !profilesController.isValidPeerId(p.trim()));
    if (invalid != null) {
      setState(() => _error = 'Room details contain an invalid peer ID');
      return;
    }

    setState(() {
      _error = null;
      _nicknameInput.text = parsed.nickname;
      _peerIds
        ..clear()
        ..addAll(parsed.peerIds
            .map((p) => p.trim())
            .where((p) => p.isNotEmpty && p != profilesController.peerId)
            .toSet());
    });
  }

  void _submit(ProfilesController profilesController) {
    final String nickname = _nicknameInput.text.trim();

    // the room must always contain the current profile's peer id
    final List<String> members = [..._peerIds];
    if (!members.contains(profilesController.peerId)) {
      members.add(profilesController.peerId);
    }

    if (profilesController.rooms.keys
        .contains(profilesController.hashRoomPeers(members))) {
      setState(() => _error = 'It appears this room already exists');
      return;
    }

    try {
      profilesController.addRoom(nickname, members);
      Navigator.pop(context);
    } on DartError catch (error) {
      setState(() => _error = 'Invalid peer ID: ${error.message}');
    }
  }

  @override
  Widget build(BuildContext context) {
    final profilesController = context.watch<ProfilesController>();

    final List<Contact> contacts = profilesController.contacts.values
        .where((c) =>
            !_peerIds.contains(c.peerId()) &&
            c.peerId() != profilesController.peerId)
        .toList();

    final bool canSubmit =
        _nicknameInput.text.trim().isNotEmpty && _peerIds.isNotEmpty;

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        TextInput(
          controller: _nicknameInput,
          labelText: 'Room name',
          focusNode: _nicknameFocusNode,
          onChanged: (_) => setState(() {}),
        ),
        const SizedBox(height: 14),
        Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Expanded(
              child: TextInput(
                controller: _peerIdInput,
                labelText: 'Peer ID',
                hintText: 'string encoded peer ID',
                onChanged: (_) => setState(() => _error = null),
                onSubmitted: (value) => _addPeerId(profilesController, value),
              ),
            ),
            const SizedBox(width: 8),
            IconButton(
              icon: SvgPicture.asset('assets/icons/Plus.svg',
                  width: 24, semanticsLabel: 'Add peer ID'),
              tooltip: 'Add peer ID',
              onPressed: () =>
                  _addPeerId(profilesController, _peerIdInput.text),
            ),
          ],
        ),
        if (contacts.isNotEmpty) ...[
          const SizedBox(height: 14),
          Row(
            children: [
              Expanded(
                child: DropDown(
                  items:
                      contacts.map((c) => (c.peerId(), c.nickname())).toList(),
                  initialSelection: contacts.firstOrNull?.peerId(),
                  onSelected: (selected) => selectedPeer = selected,
                  label: 'Contact',
                ),
              ),
              const SizedBox(width: 8),
              IconButton(
                icon: SvgPicture.asset('assets/icons/Plus.svg',
                    width: 24, semanticsLabel: 'Add contact to room'),
                tooltip: 'Add contact to room',
                onPressed: () {
                  final String? peerId =
                      selectedPeer ?? contacts.firstOrNull?.peerId();
                  if (peerId != null) {
                    _addPeerId(profilesController, peerId);
                  }
                },
              ),
            ],
          ),
        ],
        if (_peerIds.isNotEmpty) ...[
          const SizedBox(height: 14),
          Wrap(
            spacing: 8,
            runSpacing: 8,
            children: [
              for (final String peerId in _peerIds)
                _MemberChip(
                  label: profilesController.contacts[peerId]?.nickname() ??
                      _truncatePeerId(peerId),
                  onRemove: () => setState(() {
                    _peerIds.remove(peerId);
                    _error = null;
                  }),
                ),
            ],
          ),
        ],
        if (_error != null) _ErrorLine(_error!),
        const SizedBox(height: 18),
        Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            TextButton.icon(
              onPressed: () => _pasteRoomDetails(profilesController),
              icon: SvgPicture.asset('assets/icons/Copy.svg',
                  width: 18, semanticsLabel: 'Paste room details'),
              label: const Text('Paste room details'),
            ),
            const SizedBox(width: 12),
            Button(
              text: 'Create Room',
              disabled: !canSubmit,
              onPressed: () => _submit(profilesController),
            ),
          ],
        ),
      ],
    );
  }
}

String _truncatePeerId(String peerId) {
  if (peerId.length <= 16) return peerId;
  return '${peerId.substring(0, 8)}…${peerId.substring(peerId.length - 6)}';
}

/// A removable pill showing one member of the room being created.
class _MemberChip extends StatelessWidget {
  final String label;
  final VoidCallback onRemove;

  const _MemberChip({required this.label, required this.onRemove});

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.only(left: 12, right: 4, top: 4, bottom: 4),
      decoration: BoxDecoration(
        color: Theme.of(context).colorScheme.tertiaryContainer,
        borderRadius: BorderRadius.circular(16),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Text(label, style: const TextStyle(fontSize: 13)),
          const SizedBox(width: 4),
          SizedBox.square(
            dimension: 24,
            child: IconButton(
              onPressed: onRemove,
              padding: EdgeInsets.zero,
              constraints: const BoxConstraints.tightFor(width: 24, height: 24),
              iconSize: 14,
              icon: const Icon(Icons.close),
              tooltip: 'Remove',
            ),
          ),
        ],
      ),
    );
  }
}
