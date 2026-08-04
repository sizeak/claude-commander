import 'dart:async';

import 'package:claude_commander_client/chrome/lcars/elbow.dart';
import 'package:claude_commander_client/pages/activity_page.dart';
import 'package:claude_commander_client/pages/session_list_page.dart';
import 'package:claude_commander_client/state/commander_store.dart';
import 'package:claude_commander_client/state/commander_store_scope.dart';
import 'package:claude_commander_client/state/workspace_store.dart';
import 'package:claude_commander_client/theme/theme_data.dart';
import 'package:claude_commander_client/theme/tokens.dart';
import 'package:claude_commander_client/widgets/brand_mark.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_commander_api.dart';
import '../support/fixtures.dart';

/// The two views that frame themselves with a `ChromeViewRail`, under both
/// chromes. Mission Control must keep the branded header and the rounded controls
/// the pages built by hand; LCARS must draw the deck's left elbow rail, which is
/// the element's whole reason for existing.
void main() {
  late FakeCommanderApi api;
  late CommanderStore store;
  late WorkspaceStore workspace;

  setUp(() {
    api = FakeCommanderApi();
    store = CommanderStore(api: api, config: testConfig);
    workspace = WorkspaceStore.withStores([store]);
  });

  tearDown(() => workspace.dispose());

  /// A themeless `MaterialApp` falls back to Mission Control tokens, which is how
  /// every other page test pumps its subject; passing [tokens] opts into LCARS.
  Widget host(Widget body, {CommanderTokens? tokens}) => WorkspaceScope(
    workspace: workspace,
    child: MaterialApp(
      theme: tokens == null ? null : themeDataFor(tokens),
      home: Scaffold(body: body),
    ),
  );

  Widget fleet({CommanderTokens? tokens, bool showFleetHeader = true}) => host(
    SessionListBody(showFleetHeader: showFleetHeader, onSelect: (_, _) {}),
    tokens: tokens,
  );

  Widget activity({CommanderTokens? tokens}) =>
      host(const ActivityBody(), tokens: tokens);

  /// One connected server holding a single running session.
  Future<void> connect(WidgetTester tester, Widget app) async {
    api.listSessionsResponse = [sessionInfo(title: 'Alpha')];
    unawaited(store.connect());
    await tester.pumpWidget(app);
    await tester.pumpAndSettle();
  }

  group('Mission Control', () {
    testWidgets('the fleet view keeps its branded header and its controls', (
      tester,
    ) async {
      await connect(tester, fleet());

      // The header `_FleetHeader` used to build: brand mark, title, counts, ⚙.
      expect(find.byType(BrandMark), findsOneWidget);
      expect(find.text('Fleet'), findsOneWidget);
      expect(find.text('1 active · 1 total · 1 server'), findsOneWidget);
      expect(find.byIcon(Icons.settings), findsOneWidget);

      // The search box and the Recent/All toggle with its mode indicator.
      expect(find.byType(TextField), findsOneWidget);
      expect(find.text('Recent'), findsOneWidget);
      expect(find.text('All'), findsOneWidget);
      expect(find.text('grouped'), findsOneWidget);

      // And none of LCARS' chrome.
      expect(find.byType(ChromeElbow), findsNothing);
      expect(find.byType(ChromeElbowCap), findsNothing);
    });

    testWidgets('the activity view keeps its plain header and filter chips', (
      tester,
    ) async {
      await connect(tester, activity());

      expect(find.text('Activity'), findsOneWidget);
      expect(find.text('across 1 server · live'), findsOneWidget);
      // The feed is titled plainly — no brand mark, no aggregate tile.
      expect(find.byType(BrandMark), findsNothing);

      expect(find.text('All'), findsOneWidget);
      expect(find.textContaining('Needs you'), findsOneWidget);
      expect(find.text('PRs'), findsOneWidget);
      expect(find.byType(ChromeElbow), findsNothing);
    });
  });

  group('LCARS', () {
    testWidgets('the fleet view frames itself with an elbow rail', (
      tester,
    ) async {
      await connect(tester, fleet(tokens: lcarsTokens));

      // The rail: the view identifier, a block per slice, the mode readout, and
      // the settings action closing it — with the content column's cap opposite.
      expect(find.text('47-A'), findsOneWidget);
      expect(find.widgetWithText(ChromeElbow, 'RECENT'), findsOneWidget);
      expect(find.widgetWithText(ChromeElbow, 'ALL'), findsOneWidget);
      expect(find.widgetWithText(ChromeElbow, 'GROUPED'), findsOneWidget);
      expect(find.widgetWithText(ChromeElbow, 'SETTINGS'), findsOneWidget);
      expect(find.byType(ChromeElbowCap), findsOneWidget);

      // The content column carries the uppercased title and its count line.
      expect(find.text('FLEET'), findsOneWidget);
      expect(find.text('1 ACTIVE · 1 TOTAL · 1 SERVER'), findsOneWidget);
      // Mission Control's brand mark is not part of this frame.
      expect(find.byType(BrandMark), findsNothing);
    });

    testWidgets('a rail slice selects the view it names', (tester) async {
      await connect(tester, fleet(tokens: lcarsTokens));

      // All is the default, so the rail reads GROUPED; Recent reorders by age.
      await tester.tap(find.text('RECENT'));
      await tester.pumpAndSettle();

      expect(find.text('↓ RECENCY'), findsOneWidget);
      expect(find.text('GROUPED'), findsNothing);
    });

    testWidgets('the search field is a top-bordered panel, not a rounded box', (
      tester,
    ) async {
      await connect(tester, fleet(tokens: lcarsTokens));

      final field = tester.widget<TextField>(find.byType(TextField));
      expect(field.decoration!.border, InputBorder.none);
      expect(field.decoration!.filled, isFalse);
      // The decoration is the wrapping panel's instead: a 2px nav-coloured top
      // border and no radius at all, exactly as `buildPanel` draws one.
      expect(
        find.ancestor(
          of: find.byType(TextField),
          matching: find.byWidgetPredicate((widget) {
            if (widget is! Container) return false;
            final decoration = widget.decoration;
            if (decoration is! BoxDecoration) return false;
            final top = decoration.border?.top;
            return top != null &&
                top.color == lcarsTokens.nav &&
                top.width == lcarsTokens.panelTopBorder &&
                decoration.borderRadius == null;
          }),
        ),
        findsOneWidget,
      );
    });

    testWidgets('the wide shell\'s fleet pane gets no rail of its own', (
      tester,
    ) async {
      // The wide chrome already titles the column and carries the nav, so a
      // second rail inside the pane would bracket it twice.
      await connect(tester, fleet(tokens: lcarsTokens, showFleetHeader: false));

      expect(find.byType(ChromeElbowCap), findsNothing);
      expect(find.text('47-A'), findsNothing);
      expect(find.text('FLEET'), findsNothing);
      // The slices are still there — as a contiguous run of blocks, not a rail.
      expect(find.widgetWithText(ChromeElbow, 'RECENT'), findsOneWidget);
      expect(find.widgetWithText(ChromeElbow, 'ALL'), findsOneWidget);
    });

    testWidgets('the activity view frames itself with an elbow rail', (
      tester,
    ) async {
      await connect(tester, activity(tokens: lcarsTokens));

      expect(find.text('47-V'), findsOneWidget);
      expect(find.text('ACTIVITY'), findsOneWidget);
      expect(find.byType(ChromeElbowCap), findsOneWidget);
      // The feed's filters are rail blocks here, not pills.
      expect(find.widgetWithText(ChromeElbow, 'ALL'), findsOneWidget);
      expect(find.widgetWithText(ChromeElbow, 'PRS'), findsOneWidget);
      expect(find.textContaining('NEEDS YOU'), findsOneWidget);
    });

    testWidgets('an activity filter block selects its filter', (tester) async {
      await connect(tester, activity(tokens: lcarsTokens));

      await tester.tap(find.textContaining('NEEDS YOU'));
      await tester.pumpAndSettle();

      // Nothing is waiting in this fixture, so the filtered feed is empty.
      expect(find.text('Nothing needs you'), findsOneWidget);
    });
  });
}
