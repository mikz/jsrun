"""Custom fetch bridge from JavaScript to Python using jsrun Runtime."""

import asyncio
import json

from jsrun import Runtime


async def python_fetch(url):
    await asyncio.sleep(0)  # simulate async work
    return {"url": url, "status": 200, "body": "Hello from Python!"}


async def main() -> None:
    with Runtime() as rt:
        rt.bind_function("fetch", python_fetch)
        body = await rt.eval_async(
            """
            (async () => {
                const res = await fetch("https://example.com/api");
                return res.body;
            })()
            """
        )
        print("Fetched body:", body)


if __name__ == "__main__":
    asyncio.run(main())
