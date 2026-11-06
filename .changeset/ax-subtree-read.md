---
default: patch
---

Agents can pass `root` to `get_window_state` to read one numbered element's subtree, or to `act_on_element` to scope its returned state. `max_depth` now stops the accessibility traversal before deeper elements spend its budget. A shallow read can find the message pane after a long conversation list, then a rooted read can reach its contents. Partial reads keep other element numbers available, and `result: "changes"` compares only reads with the same root and depth. A stale root fails before an action runs. If an action removes the subtree requested for its result, the reply still acknowledges that the action ran.
