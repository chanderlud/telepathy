import 'package:flutter/material.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/src/rust/audio/player.dart';
import 'package:telepathy/src/rust/flutter.dart';
import 'package:telepathy/src/rust/telepathy.dart';
import 'package:telepathy/widgets/contacts/contacts_list.dart';

/// A contacts list wrapper that memoizes contact sorting.
///
/// ## Why this exists
/// `HomePage` needs to rebuild when either `SettingsController` (contacts/rooms)
/// or `StateController` (session statuses) changes. Re-sorting contacts on every
/// rebuild is wasteful because sorting is \(O(n log n)\).
///
/// This widget:
/// - Listens to `Listenable.merge([settingsController, stateController])`
/// - Caches a sorted `List<Contact>`
/// - Only re-sorts when contacts are added/removed or session status types change
///
/// ## Sorting priority
/// Connected > Connecting > Others, then alphabetical by nickname within group.
class SortedContactsList extends StatefulWidget {
  final Telepathy telepathy;
  final SettingsController settingsController;
  final StateController stateController;
  final SoundPlayer player;

  const SortedContactsList({
    super.key,
    required this.telepathy,
    required this.settingsController,
    required this.stateController,
    required this.player,
  });

  @override
  State<SortedContactsList> createState() => _SortedContactsListState();
}

class _SortedContactsListState extends State<SortedContactsList> {
  List<Contact>? _cachedSortedContacts;
  int? _previousContactsLength;
  int? _previousContactsHashCode;
  int? _previousSessionsHashCode;

  @override
  void initState() {
    super.initState();
    _refreshCacheIfNeeded(force: true);
  }

  @override
  void didUpdateWidget(covariant SortedContactsList oldWidget) {
    super.didUpdateWidget(oldWidget);

    // If controller instances are swapped, treat it as a cache invalidation.
    if (widget.settingsController != oldWidget.settingsController ||
        widget.stateController != oldWidget.stateController) {
      _cachedSortedContacts = null;
      _previousContactsLength = null;
      _previousContactsHashCode = null;
      _previousSessionsHashCode = null;
      _refreshCacheIfNeeded(force: true);
    }
  }

  List<Contact> _sortContacts() {
    final List<Contact> contacts =
        widget.settingsController.contacts.values.toList();

    // sort contacts by session status then nickname
    contacts.sort((a, b) {
      final SessionStatus aStatus = widget.stateController.sessionStatus(a);
      final SessionStatus bStatus = widget.stateController.sessionStatus(b);

      if (aStatus == bStatus) {
        return a.nickname().compareTo(b.nickname());
      } else if (aStatus.runtimeType == SessionStatus_Connected) {
        return -1;
      } else if (bStatus.runtimeType == SessionStatus_Connected) {
        return 1;
      } else if (aStatus.runtimeType == SessionStatus_Connecting) {
        return -1;
      } else if (bStatus.runtimeType == SessionStatus_Connecting) {
        return 1;
      } else {
        return 0;
      }
    });

    return contacts;
  }

  int _computeSessionsHash() {
    return Object.hashAllUnordered(
      widget.stateController.sessions.entries.map(
        (e) => Object.hash(e.key, e.value.runtimeType),
      ),
    );
  }

  void _refreshCacheIfNeeded({required bool force}) {
    final int contactsLength = widget.settingsController.contacts.length;
    final int contactsHash = Object.hashAll(
      widget.settingsController.contacts.entries.map(
        (e) => Object.hash(e.key, e.value.nickname()),
      ),
    );
    final int sessionsHash = _computeSessionsHash();

    final bool contactsChanged = _previousContactsLength != contactsLength ||
        _previousContactsHashCode != contactsHash;
    final bool sessionsChanged = _previousSessionsHashCode != sessionsHash;

    if (force ||
        _cachedSortedContacts == null ||
        contactsChanged ||
        sessionsChanged) {
      _cachedSortedContacts = _sortContacts();
      _previousContactsLength = contactsLength;
      _previousContactsHashCode = contactsHash;
      _previousSessionsHashCode = sessionsHash;
    }
  }

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: Listenable.merge([
        widget.settingsController,
        widget.stateController,
      ]),
      builder: (BuildContext context, Widget? child) {
        // Cheap change detection; only sort when inputs to ordering change
        _refreshCacheIfNeeded(force: false);

        return ContactsList(
          telepathy: widget.telepathy,
          contacts: _cachedSortedContacts ?? const <Contact>[],
          rooms: widget.settingsController.rooms.values.toList(),
          stateController: widget.stateController,
          settingsController: widget.settingsController,
          player: widget.player,
        );
      },
    );
  }
}
