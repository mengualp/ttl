# ttl — README demo cassette (live TUI traceroute)
#
# Rendered with VHS (https://github.com/charmbracelet/vhs):
#   PATH="$HOME/go/bin:$HOME/.local/bin:$PATH" vhs docs/demo.tape
#
# Captures the REAL TUI on a live trace. Raw-socket privilege comes from a
# throwaway Docker container (--cap-add=NET_RAW), so no host setcap/sudo is
# needed; the freshly built ./target/release/ttl is mounted in. Reproducible:
# `cargo build --release` then re-run the vhs command from the repo root.

Output docs/demo.gif

Set Width 1280
Set Height 760
Set FontSize 15
Set Padding 16
Set Theme "Catppuccin Mocha"
Set CursorBlink false

# --- clean prompt -------------------------------------------------------------
Hide
Type "clear"
Enter
Sleep 500ms
Show

# --- launch the real TUI on a live trace (raw sockets via Docker NET_RAW) ------
Type@35ms "docker run --rm -it --cap-add=NET_RAW -v $PWD:/w -w /w ubuntu:24.04 ./target/release/ttl 1.1.1.1"
Enter
Sleep 12s        # container start + live trace: hops appear, ASNs + rDNS resolve, sparklines build

# --- open a hop's detail view (percentiles, jitter, routing symmetry, TTL) ----
Down
Sleep 350ms
Down
Sleep 350ms
Down
Sleep 500ms
Enter
Sleep 4500ms
Escape
Sleep 3s          # hold on the fully populated table (final good frame)

# --- quit off-camera so the last frame is the TUI, not the shell prompt -------
Hide
Type "q"
Sleep 800ms
