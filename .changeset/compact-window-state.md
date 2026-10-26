---
default: minor
---

`get_window_state` and `act_on_element` take `max_depth`, which leaves out elements nested deeper and says how many. `act_on_element` takes `result`: `changes` returns only the elements that appeared, changed or went away since the window's last tree, with the window image, and `none` returns only "Done.". The default still returns the whole window state. On Calculator, a press with `changes` returned 166 bytes of text in place of 1,859.
