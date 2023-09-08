use anyhow::{anyhow, Result};
use clap::{Args, Parser, Subcommand};
use k8s_openapi::api::core::v1::{PersistentVolumeClaim, Pod, VolumeMount};
use kube::{
    api::{Api, DeleteParams, ListParams, PostParams},
    runtime::wait::{await_condition, conditions::is_pod_running},
    Client, ResourceExt,
};
use log::{debug, info};
use std::collections::BTreeMap;
use uuid::Uuid;

use handlebars::{no_escape, Handlebars};

static RAW_VOLUME_MOUNT: &str = r#"volumeMounts:
{{content}}"#;

static RAW_SECRET_MOUNT: &str = r#"      - mountPath: {{mount_path}}
        name: {{name}}
        subPath: {{sub_path}}
"#;

static RAW_VOLUME_MOUNT_PVC: &str = r#"      - mountPath: {{mount_path}}
        name: {{volume_name}}
"#;
static RAW_POD: &str = r#"
apiVersion: v1
kind: Pod
metadata:
  name: {{name}}
  namespace: {{namespace}}
  labels:
    app: resalloc-kubernetes
    has_volume: {{has_volume}}
spec:
  {{volume}}
  containers:
    - image: {{image}}
      imagePullPolicy: IfNotPresent
      name: {{name}}
      securityContext:
        privileged: {{privileged}}
      resources:
        limits:
          cpu: {{cpu}}
          memory: {{memory}}
        requests:
          cpu: {{cpu}}
          memory: {{memory}}
      {{volume_mount}}"#;
static RAW_VOLUME_HEADER: &str = "volumes:";

static RAW_VOLUME: &str = r#"
  - name: {{volume_name}}
    persistentVolumeClaim:
      claimName: {{claim_name}}"#;

static RAW_SECRET_VOLUME: &str = r#"
  - name: {{volume_name}}
    secret:
      secretName: {{secret_name}}"#;

static RAW_PVC: &str = r#"apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: {{name}}
  namespace: {{namespace}}
  labels:
    app: resalloc-kubernetes
spec:
  accessModes:
  - ReadWriteOnce
  resources:
    requests:
      storage: {{size}}
  storageClassName: {{class}}"#;

#[derive(Parser)]
#[command(name = "resalloc-kubernetes")]
#[command(author = "TommyLike <tommylikehu@gmail.com>")]
#[command(version = "1.0")]
#[command(arg_required_else_help = true)]
#[command(about = "Allocate kubernetes pod for resalloc framework", long_about = None)]
struct App {
    #[arg(long)]
    debug: bool,
    #[arg(long, global = true)]
    namespace: Option<String>,
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    #[command(about = "Create new pod resource", long_about = None)]
    Add(Box<CommandAdd>),
    #[command(about = "Delete existing pod resource by IP address", long_about = None)]
    Delete(CommandDelete),
}

#[derive(Args)]
struct CommandAdd {
    #[arg(long, default_value_t = 90)]
    #[arg(help = "timeout for waiting pod to be ready")]
    timeout: u64,
    #[arg(long)]
    #[arg(
        help = "specify the image tag used for generating, for example: docker.io/organization/image:tag"
    )]
    image_tag: String,
    #[arg(long)]
    #[arg(help = "specify the request and limit cpu resource, '1', '2000m' and etc.")]
    cpu_resource: String,
    #[arg(long)]
    #[arg(help = "specify the request and limit memory resource, '1024Mi', '2Gi' and etc.")]
    memory_resource: String,
    #[arg(long)]
    #[arg(
        help = "specify the node selector for pod resource in the format of 'NAME=VALUE', can be specified with multiple times"
    )]
    node_selector: Vec<String>,
    #[arg(long)]
    #[arg(help = "run pod in privileged mode")]
    privileged: bool,
    #[arg(long)]
    #[arg(
        help = "specify the additional labels for pod resource in the format of 'NAME=VALUE', can be specified with multiple times"
    )]
    additional_labels: Vec<String>,
    #[arg(long)]
    #[arg(
        help = "specify the additional persistent volume size, use in group(additional_volume_size, additional_volume_class, additional_volume_mount_path)."
    )]
    additional_volume_size: Option<String>,
    #[arg(long)]
    #[arg(
        help = "specify the additional persistent volume class, use in group(additional_volume_size, additional_volume_class, additional_volume_mount_path)."
    )]
    additional_volume_class: Option<String>,
    #[arg(long)]
    #[arg(
        help = "specify mount point for persistent volume, use in group(additional_volume_size, additional_volume_class, additional_volume_mount_path)."
    )]
    additional_volume_mount_path: Option<String>,
    #[arg(long, required = false)]
    #[arg(help = "just dry run and print the create resource in json")]
    dry_run: bool,
    #[arg(long, value_parser=parse_volume_mount)]
    #[arg(help = "specify secret in <mountPath>:<name>:<subPath> form")]
    secret: Option<VolumeMount>,
}

fn parse_volume_mount(value: &str) -> Result<VolumeMount, String> {
    let parts: Vec<&str> = value.split(':').collect();

    if parts.len() != 3 {
        return Err("".to_string());
    }

    Ok(VolumeMount {
        mount_path: parts[0].to_string(),
        name: parts[1].to_string(),
        sub_path: Some(parts[2].to_string()),
        mount_propagation: Default::default(),
        sub_path_expr: Default::default(),
        read_only: Default::default(),
    })
}

#[derive(Args)]
struct CommandDelete {
    #[arg(long)]
    #[arg(help = "specify ip address of pod to delete.")]
    #[arg(env = "RESALLOC_NAME")]
    name: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    let app = App::parse();
    let namespace: String = match app.namespace {
        Some(input) => input,
        None => "default".to_string(),
    };
    //log preparation
    //handle kubernetes pod resource
    match app.command {
        Some(Commands::Add(add_command)) => {
            generate_new_resource(&add_command, &namespace).await?;
        }
        Some(Commands::Delete(delete_command)) => {
            delete_resource(&delete_command, &namespace).await?;
        }
        None => {}
    };
    Ok(())
}

async fn generate_pvc_resource(
    add_command: &CommandAdd,
    namespace: &str,
    pvc_name: &str,
) -> Result<PersistentVolumeClaim> {
    let mut handler = Handlebars::new();
    handler
        .register_template_string("pvc_template", RAW_PVC)
        .unwrap();
    let mut attribute: BTreeMap<&str, String> = BTreeMap::new();
    let volume_size = add_command.additional_volume_size.clone().unwrap();
    let volume_class = add_command.additional_volume_class.clone().unwrap();
    attribute.insert("name", pvc_name.to_string());
    attribute.insert("namespace", namespace.to_string());
    attribute.insert("size", volume_size);
    attribute.insert("class", volume_class);
    let yaml = handler.render("pvc_template", &attribute).unwrap();
    Ok(serde_yaml::from_str(&yaml).unwrap())
}

fn generate_volume_str(claim_name: &str, volume_name: &str) -> Result<String> {
    let mut handler = Handlebars::new();
    handler
        .register_template_string("vol_template", RAW_VOLUME)
        .unwrap();

    let mut attribute: BTreeMap<&str, String> = BTreeMap::new();
    attribute.insert("claim_name", claim_name.to_string());
    attribute.insert("volume_name", volume_name.to_string());

    Ok(handler.render("vol_template", &attribute).unwrap())
}

fn generate_volume_secret_str(volume: &str, secret: &str) -> Result<String> {
    let mut handler = Handlebars::new();
    handler
        .register_template_string("vol_secret_template", RAW_SECRET_VOLUME)
        .unwrap();
    let mut attribute: BTreeMap<&str, String> = BTreeMap::new();
    attribute.insert("volume_name", volume.to_string());
    attribute.insert("secret_name", secret.to_string());

    Ok(handler.render("vol_secret_template", &attribute).unwrap())
}

fn generate_volume_mount_secret_str(
    mount_path: &str,
    sub_path: &str,
    name: &str,
) -> Result<String> {
    let mut handler = Handlebars::new();
    handler
        .register_template_string("vol_secret_mount_template", RAW_SECRET_MOUNT)
        .unwrap();
    let mut attribute: BTreeMap<&str, String> = BTreeMap::new();
    attribute.insert("mount_path", mount_path.to_string());
    attribute.insert("sub_path", sub_path.to_string());
    attribute.insert("name", name.to_string());

    Ok(handler
        .render("vol_secret_mount_template", &attribute)
        .unwrap())
}

fn generate_volume_mount_pvc_str(mount_path: &str, name: &str) -> Result<String> {
    let mut handler = Handlebars::new();
    handler
        .register_template_string("vol_mount_template", RAW_VOLUME_MOUNT_PVC)
        .unwrap();
    let mut attribute: BTreeMap<&str, String> = BTreeMap::new();
    attribute.insert("mount_path", mount_path.to_string());
    attribute.insert("volume_name", name.to_string());

    Ok(handler.render("vol_mount_template", &attribute).unwrap())
}

fn generate_volume_mount_str(secret_mount: &str, pvc_mount: &str) -> Result<String> {
    if secret_mount.is_empty() && pvc_mount.is_empty() {
        return Ok("".to_string());
    }

    let mut handler = Handlebars::new();
    handler
        .register_template_string("vol_mount_template", RAW_VOLUME_MOUNT)
        .unwrap();
    handler.register_escape_fn(no_escape);
    let mut content = String::from(secret_mount);
    content += pvc_mount;
    let mut attribute: BTreeMap<&str, String> = BTreeMap::new();
    attribute.insert("content", content);

    Ok(handler.render("vol_mount_template", &attribute).unwrap())
}
async fn create_simple_pod_yaml(
    add_command: &CommandAdd,
    namespace: &str,
    name: &str,
    pvc_name: &str,
    has_volume: bool,
) -> Result<String> {
    let mut handler = Handlebars::new();
    handler
        .register_template_string("pod_template", RAW_POD)
        .unwrap();
    handler.register_escape_fn(no_escape);

    let mut vol :Vec<String> = Vec::new();
    let mut vol_mount_pvc: String = Default::default();
    let mut vol_mount_secret: String = Default::default();

    if let Some(ref secret) = add_command.secret {
        vol_mount_secret = generate_volume_mount_secret_str(
            &secret.mount_path.to_string(),
            &secret.sub_path.clone().unwrap(),
            &secret.name,
        )
        .unwrap();
        vol.push(generate_volume_secret_str(&secret.name, &secret.name).unwrap());
    }
    if has_volume {
        vol.push(generate_volume_str(pvc_name, pvc_name).unwrap());
        vol_mount_pvc = generate_volume_mount_pvc_str(
            add_command.additional_volume_mount_path.as_ref().unwrap(),
            pvc_name,
        )
        .unwrap();
    }

    let vol_mount = generate_volume_mount_str(&vol_mount_secret, &vol_mount_pvc).unwrap();

    let mut attribute: BTreeMap<&str, String> = BTreeMap::new();
    attribute.insert("name", name.to_string());
    attribute.insert("namespace", namespace.to_string());
    attribute.insert("image", add_command.image_tag.clone());
    attribute.insert("cpu", add_command.cpu_resource.clone());
    attribute.insert("memory", add_command.memory_resource.clone());
    attribute.insert("privileged", add_command.privileged.to_string());
    if vol.len() != 0 {
        let mut vols :String = RAW_VOLUME_HEADER.to_string();
        for v in vol.iter() {
            vols = format!("{}{}", vols, v)
        }
        attribute.insert("volume", vols);
    }
    attribute.insert("volume_mount", vol_mount);
    attribute.insert("has_volume", has_volume.to_string());
    let s = handler.render("pod_template", &attribute).unwrap();
    debug!("render pod yaml: {}", s);
    Ok(s)
}

fn get_pvc_name(namespace: &str, additional_volume_class: &str) -> String {
    format!("resalloc-{}-{}", namespace, additional_volume_class,)
}

async fn generate_pod_resource(
    add_command: &CommandAdd,
    namespace: &str,
    name: &str,
    pvc_name: &str,
    create_volume: bool,
) -> Result<Pod> {
    let yaml =
        create_simple_pod_yaml(add_command, namespace, name, pvc_name, create_volume).await?;
    let mut pod: Pod = serde_yaml::from_str(&yaml).unwrap();

    //add labels
    if !add_command.additional_labels.is_empty() {
        let additional_labels = add_command.additional_labels.clone();
        if let Some(ref mut l) = pod.metadata.labels {
            for label in additional_labels.into_iter() {
                let pair: Vec<&str> = label.split('=').collect();
                if pair.len() == 2 {
                    l.insert(pair[0].to_string(), pair[1].to_string());
                }
            }
        }
    }

    //add node selector
    if !add_command.node_selector.is_empty() {
        if let Some(ref mut spec) = pod.spec {
            let node_selector = add_command.node_selector.clone();
            match spec.node_selector {
                Some(_) => {
                    return Err(anyhow!(
                        "generated pod resource node selector should be empty"
                    ));
                }
                None => {
                    let mut container = BTreeMap::new();
                    for s in node_selector.into_iter() {
                        let pair: Vec<&str> = s.split('=').collect();
                        if pair.len() == 2 {
                            container.insert(pair[0].to_string(), pair[1].to_string());
                        }
                    }
                    spec.node_selector = Some(container)
                }
            }
        }
    }

    Ok(pod)
}

async fn cleanup(
    pods_api: &Api<Pod>,
    pvc_api: &Api<PersistentVolumeClaim>,
    name: &str,
    additional_volume: bool,
) -> Result<()> {
    //pods unready, delete them
    delete_pod_by_name(pods_api.clone(), name).await?;
    if additional_volume {
        delete_pvc_by_name(pvc_api.clone(), name).await?;
    }
    Ok(())
}

async fn generate_new_resource(add_command: &CommandAdd, namespace: &str) -> Result<()> {
    //check persistent volume argument
    let mut additional_volume = false;
    let name = format!("resalloc-{}", Uuid::new_v4());
    let pp = PostParams::default();
    let mut pvc = None;
    let mut pvc_name = Default::default();

    if add_command.additional_volume_size.is_some()
        && add_command.additional_volume_class.is_some()
        && add_command.additional_volume_mount_path.is_some()
    {
        additional_volume = true;
        pvc_name = get_pvc_name(
            namespace,
            add_command.additional_volume_class.as_ref().unwrap(),
        );
        pvc = Some(generate_pvc_resource(add_command, namespace, &pvc_name).await?);
    }
    let pod =
        generate_pod_resource(add_command, namespace, &name, &pvc_name, additional_volume).await?;

    if add_command.dry_run {
        if pvc.is_some() {
            info!("---");
            info!("{}", serde_yaml::to_string(&pvc).unwrap());
        }
        info!("---");
        info!("{}", serde_yaml::to_string(&pod).unwrap());
        return Ok(());
    }

    let client = Client::try_default().await?;
    let pods_api: Api<Pod> = Api::namespaced(client.clone(), namespace);
    let pvc_api: Api<PersistentVolumeClaim> = Api::namespaced(client, namespace);

    // generate pvc resource
    if let Some(p) = pvc {
        pvc_api.create(&pp, &p).await?;
    }
    // generate pod resource
    pods_api.create(&pp, &pod).await?;
    //wait pod to be ready
    let running = await_condition(pods_api.clone(), &name, is_pod_running());
    match tokio::time::timeout(std::time::Duration::from_secs(add_command.timeout), running).await {
        Ok(res) => match res {
            Err(e) => {
                cleanup(&pods_api, &pvc_api, &name, additional_volume).await?;
                Err(anyhow!(
                    "failed to creating new pod resource in kubernetes, due to {:?}",
                    e
                ))
            }
            Ok(_) => {
                //check pod ip address
                match pods_api.get(&name).await {
                    Err(e) => {
                        cleanup(&pods_api, &pvc_api, &name, additional_volume).await?;
                        Err(anyhow!(
                            "failed to getting new pod resource in kubernetes, due to {:?}",
                            e
                        ))
                    }
                    Ok(current) => {
                        if let Some(status) = current.status {
                            if let Some(pod_ip) = status.pod_ip {
                                println!("{}", &pod_ip);
                                return Ok(());
                            }
                        }
                        cleanup(&pods_api, &pvc_api, &name, additional_volume).await?;
                        Err(anyhow!("container ip address empty"))
                    }
                }
            }
        },
        Err(e) => {
            cleanup(&pods_api, &pvc_api, &name, additional_volume).await?;
            Err(anyhow!(
                "failed to creating new pod resource in kubernetes, due to {:?}",
                e
            ))
        }
    }
}

async fn delete_resource(delete_command: &CommandDelete, namespace: &str) -> Result<()> {
    info!("starting to delete {} resource", &delete_command.name);
    let client = Client::try_default().await?;

    let pods_api: Api<Pod> = Api::namespaced(client.clone(), namespace);
    let pvc_api: Api<PersistentVolumeClaim> = Api::namespaced(client, namespace);

    //get pod by ip address
    let list_params =
        ListParams::default().fields(&format!("status.podIP={}", delete_command.name));
    let pods = pods_api.list(&list_params).await?;
    if pods.items.is_empty() {
        return Err(anyhow!(
            "failed to get get any pods within {} address",
            &delete_command.name
        ));
    }

    // delete pod and pvc
    for p in pods {
        if let Some(ref labels) = p.metadata.labels {
            //confirm it's created by our applications
            if let Some(app) = labels.get("app") {
                if app == "resalloc-kubernetes" {
                    delete_pod_by_name(pods_api.clone(), &p.name_any()).await?;
                    info!("pod {} has been deleted", &p.name_any());

                    //delete pvc if needed
                    if let Some(has_volume) = labels.get("has_volume") {
                        if has_volume == "true" {
                            delete_pvc_by_name(pvc_api.clone(), &p.name_any()).await?;
                            info!("pod's pvc {} has been deleted", &p.name_any());
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

async fn delete_pod_by_name(pods_api: Api<Pod>, name: &str) -> Result<()> {
    let delete_params = DeleteParams::default();
    pods_api.delete(name, &delete_params).await?;
    Ok(())
}

async fn delete_pvc_by_name(pvc_api: Api<PersistentVolumeClaim>, name: &str) -> Result<()> {
    let delete_params = DeleteParams::default();
    pvc_api.delete(name, &delete_params).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::CommandAdd;
    use crate::{generate_pod_resource, generate_pvc_resource, get_pvc_name};

    #[tokio::test]
    async fn test_pod_template_with_volume() {
        let yaml_str = r#"apiVersion: v1
kind: Pod
metadata:
  labels:
    app: resalloc-kubernetes
    has_volume: 'false'
  name: resalloc-9a1884fb-8a7b-459f-aefe-c54ac1188d71
  namespace: test_ns
spec:
  containers:
  - image: openeuler/openeuler:22.03
    imagePullPolicy: IfNotPresent
    name: resalloc-9a1884fb-8a7b-459f-aefe-c54ac1188d71
    resources:
      limits:
        cpu: 100m
        memory: 500Mi
      requests:
        cpu: 100m
        memory: 500Mi
    securityContext:
      privileged: false
"#;

        let mock_command = CommandAdd {
            timeout: 120,
            image_tag: "openeuler/openeuler:22.03".to_string(),
            cpu_resource: "100m".to_string(),
            memory_resource: "500Mi".to_string(),
            node_selector: Vec::new(),
            privileged: false,
            additional_labels: Vec::new(),
            additional_volume_class: None,
            additional_volume_size: None,
            additional_volume_mount_path: None,
            dry_run: false,
            secret: None,
        };
        let name = "resalloc-9a1884fb-8a7b-459f-aefe-c54ac1188d71";
        let namespace = "test_ns";
        let pod_generated = generate_pod_resource(&mock_command, namespace, name, "", false)
            .await
            .unwrap();

        assert_eq!(pod_generated.metadata.name.as_ref().unwrap(), name);
        assert_eq!(
            pod_generated.metadata.namespace.as_ref().unwrap(),
            namespace
        );
        assert_eq!(serde_yaml::to_string(&pod_generated).unwrap(), yaml_str);
    }

    #[tokio::test]
    async fn test_pod_template_with_volume_and_secret() {
        let yaml_str = r#"apiVersion: v1
kind: Pod
metadata:
  labels:
    app: resalloc-kubernetes
    has_volume: 'true'
  name: resalloc-9a1884fb-8a7b-459f-aefe-c54ac1188d71
  namespace: test_ns
spec:
  containers:
  - image: openeuler/openeuler:22.03
    imagePullPolicy: IfNotPresent
    name: resalloc-9a1884fb-8a7b-459f-aefe-c54ac1188d71
    resources:
      limits:
        cpu: 100m
        memory: 500Mi
      requests:
        cpu: 100m
        memory: 500Mi
    securityContext:
      privileged: false
    volumeMounts:
    - mountPath: /home/copr/server.crt
      name: copr-secrets
      subPath: server-crt
    - mountPath: /etc/test_mount
      name: resalloc-test_ns-test_pvc
  volumes:
  - name: copr-secrets
    secret:
      secretName: copr-secrets
  - name: resalloc-test_ns-test_pvc
    persistentVolumeClaim:
      claimName: resalloc-test_ns-test_pvc
"#;

        let mock_command = CommandAdd {
            timeout: 120,
            image_tag: "openeuler/openeuler:22.03".to_string(),
            cpu_resource: "100m".to_string(),
            memory_resource: "500Mi".to_string(),
            node_selector: Vec::new(),
            privileged: false,
            additional_labels: Vec::new(),
            additional_volume_class: Some("test_pvc".to_string()),
            additional_volume_size: Some("10Gi".to_string()),
            additional_volume_mount_path: Some("/etc/test_mount".to_string()),
            dry_run: false,
            secret: Some(k8s_openapi::api::core::v1::VolumeMount {
                mount_path: "/home/copr/server.crt".to_string(),
                mount_propagation: None,
                name: "copr-secrets".to_string(),
                read_only: None,
                sub_path: Some("server-crt".to_string()),
                sub_path_expr: None,
            }),
        };
        let name = "resalloc-9a1884fb-8a7b-459f-aefe-c54ac1188d71";
        let namespace = "test_ns";
        let pvc_name = get_pvc_name(
            namespace,
            mock_command.additional_volume_class.as_ref().unwrap(),
        );
        let pod_generated = generate_pod_resource(&mock_command, namespace, name, &pvc_name, true)
            .await
            .unwrap();

        assert_eq!(pod_generated.metadata.name.as_ref().unwrap(), name);
        assert_eq!(
            pod_generated.metadata.namespace.as_ref().unwrap(),
            namespace
        );
        assert_eq!(serde_yaml::to_string(&pod_generated).unwrap(), yaml_str);
    }

    #[tokio::test]
    async fn test_pod_template_without_volume() {
        let pvc_yaml_str = r#"apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  labels:
    app: resalloc-kubernetes
  name: resalloc-test_ns-test_pvc
  namespace: test_ns
spec:
  accessModes:
  - ReadWriteOnce
  resources:
    requests:
      storage: 10Gi
  storageClassName: test_pvc
"#;
        let pod_yaml_str = r#"apiVersion: v1
kind: Pod
metadata:
  labels:
    app: resalloc-kubernetes
    has_volume: 'true'
  name: resalloc-9a1884fb-8a7b-459f-aefe-c54ac1188d71
  namespace: test_ns
spec:
  containers:
  - image: openeuler/openeuler:22.03
    imagePullPolicy: IfNotPresent
    name: resalloc-9a1884fb-8a7b-459f-aefe-c54ac1188d71
    resources:
      limits:
        cpu: '1'
        memory: 500Mi
      requests:
        cpu: '1'
        memory: 500Mi
    securityContext:
      privileged: false
    volumeMounts:
    - mountPath: /etc/test_mount
      name: resalloc-test_ns-test_pvc
  volumes:
  - name: resalloc-test_ns-test_pvc
    persistentVolumeClaim:
      claimName: resalloc-test_ns-test_pvc
"#;
        let mock_command = CommandAdd {
            timeout: 120,
            image_tag: "openeuler/openeuler:22.03".to_string(),
            cpu_resource: "1".to_string(),
            memory_resource: "500Mi".to_string(),
            node_selector: Vec::new(),
            privileged: false,
            additional_labels: Vec::new(),
            additional_volume_class: Some("test_pvc".to_string()),
            additional_volume_size: Some("10Gi".to_string()),
            additional_volume_mount_path: Some("/etc/test_mount".to_string()),
            dry_run: false,
            secret: None,
        };

        let name = "resalloc-9a1884fb-8a7b-459f-aefe-c54ac1188d71";
        let namespace = "test_ns";
        let pvc_name = get_pvc_name(
            namespace,
            mock_command.additional_volume_class.as_ref().unwrap(),
        );
        let pod_generated = generate_pod_resource(&mock_command, namespace, name, &pvc_name, true)
            .await
            .unwrap();

        let pvc = generate_pvc_resource(&mock_command, namespace, &pvc_name)
            .await
            .unwrap();
        assert_eq!(pod_generated.metadata.name.as_ref().unwrap(), name);
        assert_eq!(
            pod_generated.metadata.namespace.as_ref().unwrap(),
            namespace
        );
        assert_eq!(serde_yaml::to_string(&pod_generated).unwrap(), pod_yaml_str);
        assert_eq!(serde_yaml::to_string(&pvc).unwrap(), pvc_yaml_str);
    }
}
