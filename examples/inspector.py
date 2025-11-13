"""
Minimal example showing how to debug jsrun code via the Chrome DevTools inspector.

Run this file, copy the devtools:// URL that is printed, and paste it into Chrome.
Execution will pause until the debugger attaches; hit the play button (or run
`Runtime.runIfWaitingForDebugger` in the DevTools console) to continue.
"""

from jsrun import InspectorConfig, Runtime, RuntimeConfig


def main() -> None:
    config = RuntimeConfig(inspector=InspectorConfig(display_name="Inspector Example", wait_for_connection=True))

    with Runtime(config) as runtime:
        endpoints = runtime.inspector_endpoints()
        if not endpoints:
            raise RuntimeError("Inspector is not enabled for this runtime")

        print("Inspector websocket:", endpoints.websocket_url)
        print("Chrome DevTools URL:", endpoints.devtools_frontend_url)
        print("\nAttach Chrome DevTools using either:")
        print("  - Open the devtools:// URL above in Chrome")
        print("  - Or use chrome://inspect and click 'inspect' under 'Remote Target'")
        print("\nWaiting for debugger... (will block on first eval until you click Resume/F8)\n")

        runtime.eval("globalThis.counter = 0;")
        print("Debugger attached! Running demo code...\n")
        for _ in range(3):
            print("counter ->", runtime.eval("++counter"))

        print("Triggering a manual breakpoint via `debugger;`")
        runtime.eval("debugger; counter += 1; counter;")


if __name__ == "__main__":
    main()
