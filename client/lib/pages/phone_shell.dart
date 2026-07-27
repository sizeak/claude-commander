import 'package:flutter/material.dart';

import '../state/commander_store_scope.dart';
import '../theme/app_colors.dart';
import '../theme/app_theme.dart';
import 'activity_page.dart';
import 'session_list_page.dart';

/// The redesigned phone root: a two-tab bottom-nav shell over the Fleet list and
/// the Activity feed, with a raised centre FAB that starts a new session. Both
/// tabs are kept alive in an [IndexedStack] so switching between them preserves
/// each view's scroll position, search text, and filter state.
///
/// Reuses the same layout-agnostic bodies as the wide shell — [SessionListBody]
/// (with its branded Fleet header enabled) and [ActivityBody] — and the shared
/// [openSessionDetail] / [openCreateSession] helpers, so navigation, session
/// creation, and the servers/projects/programs settings menu all behave exactly
/// as they do elsewhere. Settings live behind the ⚙ in the Fleet header.
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
    return Scaffold(
      body: SafeArea(
        bottom: false,
        child: IndexedStack(
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
      floatingActionButton: FloatingActionButton(
        onPressed: () => openCreateSession(context, workspace),
        backgroundColor: AppColors.accent,
        foregroundColor: AppColors.bg,
        elevation: 6,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(16),
        ),
        tooltip: 'New session',
        child: const Icon(Icons.add, size: 26),
      ),
      floatingActionButtonLocation: FloatingActionButtonLocation.centerDocked,
      bottomNavigationBar: BottomAppBar(
        color: AppColors.bgRaised,
        height: 62,
        padding: EdgeInsets.zero,
        child: SafeArea(
          top: false,
          child: Row(
            children: [
              Expanded(
                child: _NavTab(
                  glyph: '▤',
                  label: 'FLEET',
                  selected: _index == 0,
                  onTap: () => _go(0),
                ),
              ),
              const SizedBox(width: 64),
              Expanded(
                child: _NavTab(
                  glyph: '≋',
                  label: 'ACTIVITY',
                  selected: _index == 1,
                  onTap: () => _go(1),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// One bottom-nav tab: the deck's glyph over a mono uppercase label, tinted
/// accent when active and muted otherwise.
class _NavTab extends StatelessWidget {
  final String glyph;
  final String label;
  final bool selected;
  final VoidCallback onTap;

  const _NavTab({
    required this.glyph,
    required this.label,
    required this.selected,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    final color = selected ? AppColors.accent : AppColors.textFaint;
    return InkWell(
      onTap: onTap,
      child: Column(
        mainAxisAlignment: MainAxisAlignment.center,
        mainAxisSize: MainAxisSize.min,
        children: [
          Text(glyph, style: TextStyle(fontSize: 17, color: color, height: 1)),
          const SizedBox(height: 4),
          Text(
            label,
            style: AppTheme.mono(size: 9, weight: FontWeight.w600, color: color),
          ),
        ],
      ),
    );
  }
}
