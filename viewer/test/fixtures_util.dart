import 'dart:convert';
import 'dart:io';

/// Fixture files live in test/fixtures (copied from protocol/examples).
Directory get fixturesDir => Directory('test/fixtures');

String fixtureText(String name) =>
    File('${fixturesDir.path}/$name').readAsStringSync();

Map<String, dynamic> fixtureJson(String name) =>
    jsonDecode(fixtureText(name)) as Map<String, dynamic>;

List<File> fixtureFiles(String prefix) =>
    fixturesDir
        .listSync()
        .whereType<File>()
        .where(
          (f) =>
              f.uri.pathSegments.last.startsWith(prefix) &&
              f.path.endsWith('.json'),
        )
        .toList()
      ..sort((a, b) => a.path.compareTo(b.path));
