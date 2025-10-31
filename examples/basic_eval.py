"""Basic usage of the jsrun Runtime."""

from jsrun import Runtime


def main() -> None:
    with Runtime() as rt:
        print("2 + 2 =", rt.eval("2 + 2"))
        rt.eval("globalThis.counter = 0;")
        for _ in range(3):
            print("counter ->", rt.eval("++counter"))


if __name__ == "__main__":
    main()
