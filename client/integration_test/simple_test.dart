import 'package:claude_commander_client/main.dart';
import 'package:claude_commander_client/src/rust/frb_generated.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  setUpAll(() async => await RustLib.init());

  testWidgets('boots to the connection form with the Rust bridge initialised', (
    tester,
  ) async {
    await tester.pumpWidget(const CommanderApp(initialConfig: null));
    expect(find.text('Connect to server'), findsOneWidget);
  });
}
