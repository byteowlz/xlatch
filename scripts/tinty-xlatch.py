#!/usr/bin/env python3
"""Tinty 0.29+ hook: export its palette environment atomically as xlatch JSON."""
import json
import os
from pathlib import Path
import re
import tempfile


def palette(environment):
    system = environment.get("TINTY_SCHEME_SYSTEM")
    mode = environment.get("TINTY_SCHEME_VARIANT")
    if system not in ("base16", "base24") or mode not in ("light", "dark"):
        raise ValueError("Run as a Tinty 0.29+ hook with scheme environment variables")
    slots = {}
    for index in range(24 if system == "base24" else 16):
        slot = f"base{index:02X}"
        value = "".join(environment.get(f"TINTY_SCHEME_PALETTE_{slot.upper()}_HEX_{channel}", "") for channel in "RGB")
        if not re.fullmatch(r"[0-9a-fA-F]{6}", value):
            raise ValueError(f"Missing or invalid Tinty color: {slot}")
        slots[slot] = "#" + value
    return {"name": environment.get("TINTY_SCHEME_ID", "Tinted"), "system": system, "mode": mode, "slots": slots}


def main():
    theme = palette(os.environ)
    config = Path(os.environ.get("XDG_CONFIG_HOME", str(Path.home() / ".config")))
    if not config.is_absolute():
        raise ValueError("XDG_CONFIG_HOME must be absolute")
    destination = config / "xlatch" / "theme.json"
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(mode="w", dir=destination.parent, delete=False) as output:
            temporary = Path(output.name)
            json.dump(theme, output, indent=2)
            output.write("\n")
        os.replace(temporary, destination)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


if __name__ == "__main__":
    main()
