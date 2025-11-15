# jsrun Examples

This directory contains practical examples demonstrating jsrun's features and use cases.

## Getting Started

### Basic Usage

- [**basic_eval.py**](basic_eval.py) - Introduction to jsrun with both context-local API and explicit Runtime usage
- [**context_local_api.py**](context_local_api.py) - Comprehensive guide to the context-local API with automatic runtime management

### Bindings

- [**bindings_basic.py**](bindings_basic.py) - Exposing Python functions and data to JavaScript
- [**bindings_async.py**](bindings_async.py) - Async function bindings and decorator-style syntax

### Type System

- [**type_conversion.py**](type_conversion.py) - Reference for type conversion between Python and JavaScript

## Advanced Features

### Modules

- [**modules_basic.py**](modules_basic.py) - ES6 module loading with static modules and custom resolvers
- [**modules_wasm.py**](modules_wasm.py) - Loading and executing WebAssembly modules

### Performance

- [**snapshot_example.py**](snapshot_example.py) - Pre-initialize V8 for faster startup times

### Security & Resource Control

- [**resource_limits.py**](resource_limits.py) - Memory limits, timeouts, and safe execution of untrusted code

### Debugging

- [**inspector.py**](inspector.py) - Chrome DevTools integration for debugging JavaScript execution

## Integration Examples

### Web Applications

- [**fastapi_multitenant.py**](fastapi_multitenant.py) - Multi-tenant JavaScript execution with FastAPI, resource limits, and error handling

### External Libraries

- [**markdown_parser.py**](markdown_parser.py) - Load external JS libraries (marked.js) from CDN and expose async functions to Python

### Concurrency

- [**threading_gil.py**](threading_gil.py) - Per-thread runtime isolation and GIL release demonstration

## Running Examples

All examples can be run directly with Python:

```bash
python examples/basic_eval.py
python examples/context_local_api.py
python examples/bindings_basic.py
# ... etc
```

Or using uv:

```bash
uv run python examples/basic_eval.py
```
