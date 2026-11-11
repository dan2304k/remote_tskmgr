import '../core/host_registry.dart';

/// Dashboard order: hosts whose id is in [order] first, in that order; then the rest in
/// their given (automatic) order. Unknown ids in [order] are ignored.
List<HostState> orderHosts(List<HostState> hosts, List<String> order) {
  if (order.isEmpty) return hosts;
  final byId = {for (final h in hosts) h.id: h};
  final out = <HostState>[];
  final used = <String>{};
  for (final id in order) {
    final h = byId[id];
    if (h != null && used.add(id)) out.add(h);
  }
  for (final h in hosts) {
    if (!used.contains(h.id)) out.add(h);
  }
  return out;
}

/// [ids] with [dragged] moved into the slot currently held by [target] (both must be
/// in [ids]; otherwise [ids] is returned unchanged).
List<String> dropInto(List<String> ids, String dragged, String target) {
  final to = ids.indexOf(target);
  if (to < 0 || !ids.contains(dragged) || dragged == target) return ids;
  final next = [...ids]..remove(dragged);
  next.insert(to.clamp(0, next.length), dragged);
  return next;
}
