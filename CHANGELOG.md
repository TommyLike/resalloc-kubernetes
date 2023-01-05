# Changelog

## Resalloc-kubernetes 1.0.0 (2023-01-05)

### Added
- Support `add` command for allocating new pod resource for resalloc framework, options including image tag, 
  operation timeout, namespace, cpu resource, memory resource, node selector, privileged mode, additional pod label, and additional volume. 
- Support `delete` command for deleting pod resource by ip address allocated by resalloc-kubernetes command.
- Support x86 and aarch64 platform.
