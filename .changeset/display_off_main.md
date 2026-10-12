
Displays you make with **New Display** are made off the main thread too. If an agent's session ends, or a parallel call of the same session gets a display, while macOS is still making one, that one is dropped instead of left running.
