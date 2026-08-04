import 'package:flutter/material.dart';

import '../chrome/chrome.dart';
import '../chrome/chrome_forms.dart';
import '../state/commander_store_scope.dart';
import 'activity_page.dart';
import 'session_list_page.dart';

/// The phone root: a two-tab shell over the Fleet list and the Activity feed,
/// with a prominent centre action that starts a new session. Both tabs are kept
/// alive in an [IndexedStack] so switching preserves each view's scroll position,
/// search text, and filter state.
///
/// The frame itself comes from [ChromeShell], because the two themes build it
/// very differently — Mission Control docks a `FloatingActionButton` over a
/// `BottomAppBar`, while LCARS has neither and renders three contiguous footer
/// blocks instead. This page only says *what* the destinations are.
///
/// Reuses the same layout-agnostic bodies as the wide shell — [SessionListBody]
/// (with its branded Fleet header enabled) and [ActivityBody] — and the shared
/// [openSessionDetail] / [openCreateSession] helpers, so navigation, session
/// creation, and settings all behave as they do elsewhere.
class PhoneShell extends StatefulWidget {
  const PhoneShell({super.key});

  @override
  State<PhoneShell> createState() => _PhoneShellState();
}

class _PhoneShellState extends State<PhoneShell> {
  int _index = 0;

  void _go(int index) => setState(() => _index = index);

  @override
  Widget build(BuildContext context) {
    final workspace = WorkspaceScope.of(context)!;
    return ChromeShell(
      ChromeShellSpec(
        items: [
          ChromeNavItem(
            label: 'FLEET',
            glyph: '▤',
            selected: _index == 0,
            onTap: () => _go(0),
          ),
          ChromeNavItem(
            label: 'ACTIVITY',
            glyph: '≋',
            selected: _index == 1,
            onTap: () => _go(1),
          ),
        ],
        centreAction: ChromeButtonAction(
          icon: Icons.add,
          label: 'New session',
          onPressed: () => openCreateSession(context, workspace),
        ),
        body: IndexedStack(
          index: _index,
          children: [
            SessionListBody(
              showFleetHeader: true,
              onSelect: (store, session) =>
                  openSessionDetail(context, store, session),
            ),
            const ActivityBody(),
          ],
        ),
      ),
    );
  }
}
