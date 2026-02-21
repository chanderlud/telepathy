import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/src/rust/flutter.dart';
import 'package:telepathy/widgets/contacts/contacts_list.dart';

/// A contacts list wrapper that memoizes contact sorting.
///
/// ## Why this exists
/// `HomePage` needs to rebuild when either `SettingsController` (contacts/rooms)
/// or `StateController` (session statuses) changes. Re-sorting contacts on every
/// rebuild is wasteful because sorting is \(O(n log n)\).
///
/// This widget:
/// - Listens to `ProfilesController` and `StateController` via provider
/// - Caches a sorted `List<Contact>`
/// - Only re-sorts when contacts are added/removed or session status types change
///
/// ## Sorting priority
/// Connected > Connecting > Others, then alphabetical by nickname within group.
class SortedContactsList extends StatefulWidget {
  const SortedContactsList({super.key});

  @override
  State<SortedContactsList> createState() => _SortedContactsListState();
}

class _SortedContactsListState extends State<SortedContactsList> {
  List<Contact>? _cachedSortedContacts;
  int? _previousContactsLength;
  int? _previousContactsHashCode;
  int? _previousSessionsHashCode;

  List<Contact> _sortContacts(
      ProfilesController profilesController, StateController stateController) {
    final List<Contact> contacts =
        profilesController.contacts.values.toList();

    // sort contacts by session status then nickname
    contacts.sort((a, b) {
      final SessionStatus aStatus = stateController.sessionStatus(a);
      final SessionStatus bStatus = stateController.sessionStatus(b);

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

  int _computeSessionsHash(StateController stateController) {
    return Object.hashAllUnordered(
      stateController.sessions.entries.map(
        (e) => Object.hash(e.key, e.value.runtimeType),
      ),
    );
  }

  void _refreshCacheIfNeeded(ProfilesController profilesController,
      StateController stateController,
      {required bool force}) {
    final int contactsLength = profilesController.contacts.length;
    final int contactsHash = Object.hashAll(
      profilesController.contacts.entries.map(
        (e) => Object.hash(e.key, e.value.nickname()),
      ),
    );
    final int sessionsHash = _computeSessionsHash(stateController);

    final bool contactsChanged = _previousContactsLength != contactsLength ||
        _previousContactsHashCode != contactsHash;
    final bool sessionsChanged = _previousSessionsHashCode != sessionsHash;

    if (force ||
        _cachedSortedContacts == null ||
        contactsChanged ||
        sessionsChanged) {
      _cachedSortedContacts =
          _sortContacts(profilesController, stateController);
      _previousContactsLength = contactsLength;
      _previousContactsHashCode = contactsHash;
      _previousSessionsHashCode = sessionsHash;
    }
  }

  @override
  Widget build(BuildContext context) {
    final profilesController = context.watch<ProfilesController>();
    final stateController = context.watch<StateController>();

    // Cheap change detection; only sort when inputs to ordering change
    _refreshCacheIfNeeded(profilesController, stateController, force: false);

    return ContactsList(
      contacts: _cachedSortedContacts ?? const <Contact>[],
      rooms: profilesController.rooms.values.toList(),
    );
  }
}
