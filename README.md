# Resalloc kubernetes
resalloc-kubernetes used for generating in cluster pod resource for [COPR](https://copr.fedorainfracloud.org/) cluster.

# Generate pod
command would be:
```console
Create new pod resource

Usage: resalloc-kubernetes add [OPTIONS] --image-tag <IMAGE_TAG> --cpu-resource <CPU_RESOURCE> --memory-resource <MEMORY_RESOURCE>

Options:
      --timeout <TIMEOUT>
          timeout for waiting pod to be ready [default: 60]
      --image-tag <IMAGE_TAG>
          specify the image tag used for generating, for example: docker.io/organization/image:tag
      --namespace <NAMESPACE>
       
      --cpu-resource <CPU_RESOURCE>
          specify the request and limit cpu resource, '1', '2000m' and etc.
      --memory-resource <MEMORY_RESOURCE>
          specify the request and limit memory resource, '1024Mi', '2Gi' and etc.
      --node-selector <NODE_SELECTOR>
          specify the node selector for pod resource in the format of 'NAME=VALUE', can be specified with multiple times
      --privileged
          run pod in privileged mode

      --additional-labels <ADDITIONAL_LABELS>
          specify the additional labels for pod resource in the format of 'NAME=VALUE', can be specified with multiple times
      --additional-volume-size <ADDITIONAL_VOLUME_SIZE>
          specify the additional persistent volume size, use in group(additional_volume_size, additional_volume_class, additional_volume_mount_path).
      --additional-volume-class <ADDITIONAL_VOLUME_CLASS>
          specify the additional persistent volume class, use in group(additional_volume_size, additional_volume_class, additional_volume_mount_path).
      --additional-volume-mount-path <ADDITIONAL_VOLUME_MOUNT_PATH>
          specify mount point for persistent volume, use in group(additional_volume_size, additional_volume_class, additional_volume_mount_path).
  -h, --help
          Print help information

```
# Remove pod
command would be:
````console
Delete existing pod resource by IP address

Usage: resalloc-kubernetes delete [OPTIONS] --name <NAME>

Options:
      --name <NAME>            specify ip address of pod to delete. [env: RESALLOC_NAME=]
      --namespace <NAMESPACE>  
  -h, --help                   Print help information

````