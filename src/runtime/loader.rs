//! Module loader that delegates resolution and loading to Python callables.

use deno_core::{
    ModuleLoader, ModuleSource, ModuleSourceCode, ModuleSpecifier, ModuleType, RequestedModuleType,
};
use deno_error::JsErrorBox;
use futures::FutureExt;
use pyo3::prelude::*;
use pyo3_async_runtimes::TaskLocals;
use std::cell::RefCell;

#[derive(Default)]
struct LoaderInner {
    static_modules: std::collections::HashMap<String, String>,
    resolver: Option<Py<PyAny>>,
    loader: Option<Py<PyAny>>,
}

pub struct PythonModuleLoader {
    inner: RefCell<LoaderInner>,
    task_locals: RefCell<Option<TaskLocals>>,
}

impl PythonModuleLoader {
    pub fn new() -> Self {
        Self {
            inner: RefCell::new(LoaderInner::default()),
            task_locals: RefCell::new(None),
        }
    }

    pub fn set_resolver(&self, resolver: Py<PyAny>) {
        self.inner.borrow_mut().resolver = Some(resolver);
    }

    pub fn set_loader(&self, loader: Py<PyAny>) {
        self.inner.borrow_mut().loader = Some(loader);
    }

    pub fn add_static_module(&self, name: String, source: String) {
        self.inner.borrow_mut().static_modules.insert(name, source);
    }

    pub fn set_task_locals(&self, task_locals: TaskLocals) {
        *self.task_locals.borrow_mut() = Some(task_locals);
    }

    fn resolve_static(&self, specifier: &str) -> Option<String> {
        // Strip "static:" prefix if present
        let bare_specifier = specifier.strip_prefix("static:").unwrap_or(specifier);

        // Check if this is a static module (bare specifier in our registry)
        if self
            .inner
            .borrow()
            .static_modules
            .contains_key(bare_specifier)
        {
            // Return a synthetic URL using jsrun: scheme to make it valid
            Some(format!("jsrun://static/{}", bare_specifier))
        } else {
            None
        }
    }
}

impl ModuleLoader for PythonModuleLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _kind: deno_core::ResolutionKind,
    ) -> Result<deno_core::url::Url, JsErrorBox> {
        // Handle jsrun://runtime/module_name - strip the base and treat as bare specifier
        let actual_specifier = if let Some(bare) = specifier.strip_prefix("jsrun://runtime/") {
            bare
        } else {
            specifier
        };

        let inner = self.inner.borrow();

        // If we have a custom resolver, use it
        if let Some(ref resolver) = inner.resolver {
            return Python::attach(|py| {
                let result = resolver
                    .call1(py, (actual_specifier, referrer))
                    .map_err(|e| {
                        JsErrorBox::new(
                            "Error",
                            format!("Module resolution failed for {}: {}", actual_specifier, e),
                        )
                    })?;

                // Check if the result is None
                if result.is_none(py) {
                    // Python returned None, fall back to static resolution
                    if let Some(static_spec) = self.resolve_static(actual_specifier) {
                        return ModuleSpecifier::parse(&static_spec)
                            .map_err(|e| JsErrorBox::new("URIError", e.to_string()));
                    } else {
                        // Fallback failed, deny
                        return Err(JsErrorBox::new(
                            "Error",
                            format!("Module resolution denied for {}. Resolver returned None and no static module was found.", actual_specifier)
                        ));
                    }
                }

                // Python returned a string
                let resolved_str = result.extract::<String>(py).map_err(|e| {
                    JsErrorBox::new(
                        "TypeError",
                        format!("Module resolver must return a string or None: {}", e),
                    )
                })?;

                // Empty string also means fall back to static resolution
                if resolved_str.is_empty() {
                    if let Some(static_spec) = self.resolve_static(actual_specifier) {
                        return ModuleSpecifier::parse(&static_spec)
                            .map_err(|e| JsErrorBox::new("URIError", e.to_string()));
                    } else {
                        // Fallback failed, deny
                        return Err(JsErrorBox::new(
                            "Error",
                            format!("Module resolution denied for {}. Resolver returned empty string and no static module was found.", actual_specifier)
                        ));
                    }
                }

                ModuleSpecifier::parse(&resolved_str)
                    .map_err(|e| JsErrorBox::new("URIError", e.to_string()))
            });
        }

        // No custom resolver, only allow static modules
        if let Some(static_spec) = self.resolve_static(actual_specifier) {
            ModuleSpecifier::parse(&static_spec)
                .map_err(|e| JsErrorBox::new("URIError", e.to_string()))
        } else {
            Err(JsErrorBox::new(
                "Error",
                format!(
                    "Module resolution denied for {}. Did you call add_static_module()?",
                    actual_specifier
                ),
            ))
        }
    }

    fn load(
        &self,
        module_specifier: &deno_core::url::Url,
        _maybe_referrer: Option<&deno_core::url::Url>,
        _is_dyn_import: bool,
        _requested_module_type: RequestedModuleType,
    ) -> deno_core::ModuleLoadResponse {
        let specifier = module_specifier.as_str();
        let inner = self.inner.borrow();

        // Handle static modules (jsrun://static/module_name)
        if specifier.starts_with("jsrun://static/") {
            let name = specifier.strip_prefix("jsrun://static/").unwrap();
            if let Some(source) = inner.static_modules.get(name) {
                let module = ModuleSource::new(
                    ModuleType::JavaScript,
                    ModuleSourceCode::String(source.clone().into()),
                    module_specifier,
                    None,
                );
                return deno_core::ModuleLoadResponse::Async(
                    async move { Ok(module) }.boxed_local(),
                );
            }
        }

        // If we have a custom loader, use it - clone before entering async block
        let loader_opt = inner
            .loader
            .as_ref()
            .map(|l| Python::attach(|py| l.clone_ref(py)));
        drop(inner); // Drop the borrow before the async block

        if let Some(loader_clone) = loader_opt {
            let task_locals = self.task_locals.borrow().clone();
            let specifier_string = specifier.to_string();
            let module_specifier_owned = module_specifier.clone();

            return deno_core::ModuleLoadResponse::Async(Box::pin(async move {
                let source_obj = if let Some(ref locals) = task_locals {
                    // Call the loader with task locals for async support
                    let result_and_is_coro = Python::attach(|py| {
                        let result = loader_clone
                            .bind(py)
                            .call1((specifier_string.clone(),))
                            .map_err(|e| {
                                JsErrorBox::new(
                                    "Error",
                                    format!("Failed to call module loader: {}", e),
                                )
                            })?;

                        // Check if the result is a coroutine
                        let inspect = py.import("inspect").map_err(|e| {
                            JsErrorBox::new(
                                "Error",
                                format!("Failed to import inspect module: {}", e),
                            )
                        })?;
                        let is_coroutine = inspect
                            .call_method1("iscoroutine", (&result,))
                            .map_err(|e| {
                                JsErrorBox::new(
                                    "Error",
                                    format!("Failed to check if result is coroutine: {}", e),
                                )
                            })?
                            .extract::<bool>()
                            .map_err(|e| {
                                JsErrorBox::new(
                                    "Error",
                                    format!(
                                        "Failed to extract boolean from iscoroutine check: {}",
                                        e
                                    ),
                                )
                            })?;

                        Ok::<_, JsErrorBox>((result.unbind(), is_coroutine))
                    })?;

                    if result_and_is_coro.1 {
                        // Result is a coroutine, convert to future and await
                        let py_future = Python::attach(|py| {
                            let bound_result = result_and_is_coro.0.clone_ref(py).into_bound(py);
                            pyo3_async_runtimes::into_future_with_locals(locals, bound_result)
                                .map_err(|e| {
                                    JsErrorBox::new(
                                        "Error",
                                        format!("Failed to create async future: {}", e),
                                    )
                                })
                        })?;

                        py_future.await.map_err(|e| {
                            JsErrorBox::new("Error", format!("Module loader failed: {}", e))
                        })?
                    } else {
                        // Result is synchronous (plain string), use it directly
                        result_and_is_coro.0
                    }
                } else {
                    // Synchronous call - call directly
                    Python::attach(|py| {
                        let result = loader_clone
                            .bind(py)
                            .call1((specifier_string.clone(),))
                            .map_err(|e| {
                                JsErrorBox::new(
                                    "Error",
                                    format!("Failed to call module loader: {}", e),
                                )
                            })?;

                        // Check if the result is a coroutine
                        let inspect = py.import("inspect").map_err(|e| {
                            JsErrorBox::new(
                                "Error",
                                format!("Failed to import inspect module: {}", e),
                            )
                        })?;
                        let is_coroutine = inspect
                            .call_method1("iscoroutine", (&result,))
                            .map_err(|e| {
                                JsErrorBox::new(
                                    "Error",
                                    format!("Failed to check if result is coroutine: {}", e),
                                )
                            })?
                            .extract::<bool>()
                            .map_err(|e| {
                                JsErrorBox::new(
                                    "Error",
                                    format!(
                                        "Failed to extract boolean from iscoroutine check: {}",
                                        e
                                    ),
                                )
                            })?;

                        if is_coroutine {
                            // Close the coroutine to prevent "never awaited" warning
                            let _ = result.call_method0("close");
                            return Err(JsErrorBox::new(
                                "TypeError",
                                "An async module loader cannot be used with a synchronous evaluation (eval_module). Use eval_module_async() instead."
                            ));
                        }

                        Ok::<_, JsErrorBox>(result.unbind())
                    })?
                };

                let source: String = Python::attach(|py| {
                    source_obj.extract(py).map_err(|e| {
                        JsErrorBox::new(
                            "TypeError",
                            format!("Module loader must return a string: {}", e),
                        )
                    })
                })?;

                let module = ModuleSource::new(
                    ModuleType::JavaScript,
                    ModuleSourceCode::String(source.into()),
                    &module_specifier_owned,
                    None,
                );
                Ok(module)
            }));
        }

        // No loader available
        deno_core::ModuleLoadResponse::Sync(Err(JsErrorBox::new(
            "Error",
            format!(
                "Module loading denied for {}. Did you call set_module_loader()?",
                specifier
            ),
        )))
    }
}
