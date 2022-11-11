use std::collections::BTreeMap;
use k8s_openapi::api::core::v1::{Pod, PersistentVolumeClaim};
use uuid::Uuid;
use clap::{
    Args, Parser, Subcommand
};
use kube::{
    Client,
    api::{Api, ListParams, DeleteParams, PostParams},
    ResourceExt,
    runtime::wait::{await_condition, conditions::is_pod_running}
};
use anyhow::{anyhow, Result};

use handlebars::Handlebars;

static RAW_POD: &str = r#"
{
	"apiVersion": "v1",
	"kind": "Pod",
	"metadata": {
		"name": "{{name}}",
		"namespace": "{{namespace}}",
		"labels": {
			"app": "resalloc-kubernetes"
		}
	},
	"spec": {
		"containers": [{
			"image": "{{image}}",
			"imagePullPolicy": "Always",
			"name": "resalloc-ssh",
			"securityContext": {
			    "privileged": {{privileged}}
			    },
			"resources": {
				"limits": {
					"cpu": "{{cpu}}",
					"memory": "{{memory}}"
				},
				"requests": {
					"cpu": "{{cpu}}",
					"memory": "{{memory}}"
				}
			}
		}]
	}
}"#;

static RAW_POD_WITHIN_VOLUME: &str = r#"
{
	"apiVersion": "v1",
	"kind": "Pod",
	"metadata": {
		"name": "{{name}}",
		"namespace": "{{namespace}}",
		"labels": {
			"app": "resalloc-kubernetes",
			"has_volume": "true"
		}
	},
	"spec": {
		"containers": [{
			"image": "{{image}}",
			"imagePullPolicy": "Always",
			"name": "resalloc-ssh",
			"securityContext": {
			    "privileged": {{privileged}}
			    },
			"volumeMounts": [{
			    "mountPath": "{{mount_path}}",
			    "name": "{{volume_name}}"
			}],
			"resources": {
				"limits": {
					"cpu": "{{cpu}}",
					"memory": "{{memory}}"
				},
				"requests": {
					"cpu": "{{cpu}}",
					"memory": "{{memory}}"
				}
			}
		}],
		"volumes": [{
			    "persistentVolumeClaim": {
			         "claimName": "{{claim_name}}"
			     },
			    "name": "{{volume_name}}"
			}]
	}
}"#;

static RAW_PVC: &str = r#"
{
    "apiVersion": "v1",
    "kind": "PersistentVolumeClaim",
    "metadata": {
        "name": "{{name}}",
        "namespace": "{{namespace}}",
        "labels": {
            "app": "resalloc-kubernetes"
        }
    },
    "spec": {
        "accessModes": [
            "ReadWriteOnce"
        ],
        "resources": {
            "requests": {
                "storage": "{{size}}"
            }
        },
        "storageClassName": "{{class}}"
    }
}
"#;

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
    command: Option<Commands>
}

#[derive(Subcommand)]
enum Commands {
    #[command(about = "Create new pod resource", long_about = None)]
    Add(CommandAdd),
    #[command(about = "Delete existing pod resource by IP address", long_about = None)]
    Delete(CommandDelete)
}

#[derive(Args)]
struct CommandAdd {
    #[arg(long, default_value_t = 90)]
    #[arg(help = "timeout for waiting pod to be ready")]
    timeout: u64,
    #[arg(long)]
    #[arg(help = "specify the image tag used for generating, for example: docker.io/organization/image:tag")]
    image_tag: String,
    #[arg(long)]
    #[arg(help = "specify the request and limit cpu resource, '1', '2000m' and etc.")]
    cpu_resource: String,
    #[arg(long)]
    #[arg(help = "specify the request and limit memory resource, '1024Mi', '2Gi' and etc.")]
    memory_resource: String,
    #[arg(long)]
    #[arg(help = "specify the node selector for pod resource in the format of 'NAME=VALUE', can be specified with multiple times")]
    node_selector: Vec<String>,
    #[arg(long)]
    #[arg(help = "run pod in privileged mode")]
    privileged: bool,
    #[arg(long)]
    #[arg(help = "specify the additional labels for pod resource in the format of 'NAME=VALUE', can be specified with multiple times")]
    additional_labels: Vec<String>,
    #[arg(long)]
    #[arg(help = "specify the additional persistent volume size, use in group(additional_volume_size, additional_volume_class, additional_volume_mount_path).")]
    additional_volume_size: Option<String>,
    #[arg(long)]
    #[arg(help = "specify the additional persistent volume class, use in group(additional_volume_size, additional_volume_class, additional_volume_mount_path).")]
    additional_volume_class: Option<String>,
    #[arg(long)]
    #[arg(help = "specify mount point for persistent volume, use in group(additional_volume_size, additional_volume_class, additional_volume_mount_path).")]
    additional_volume_mount_path: Option<String>,
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
    let app = App::parse();
    let client = Client::try_default().await?;
    let namespace: String = match app.namespace {
        Some(input) => {
            input
        },
        None => {
            "default".to_string()
        }
    };
    //log preparation
    //handle kubernetes pod resource
    match app.command {
        Some(Commands::Add(add_command)) => {
            generate_new_resource(client.clone(), add_command, &namespace).await.unwrap();
        }
        Some(Commands::Delete(delete_command)) => {
            delete_resource(client.clone(), delete_command, &namespace).await.unwrap();
        }
        None => {
        }
    };
    Ok(())
}

async fn generate_pvc_resource(add_command: &CommandAdd, namespace: &str, name: &str) -> Result<PersistentVolumeClaim> {
    let mut handler = Handlebars::new();
    handler.register_template_string("pvc_template", RAW_PVC).unwrap();
    let mut attribute:BTreeMap<&str, String> = BTreeMap::new();
    let volume_size = add_command.additional_volume_size.clone().unwrap();
    let volume_class = add_command.additional_volume_class.clone().unwrap();
    attribute.insert("name", name.to_string());
    attribute.insert("namespace", namespace.to_string());
    attribute.insert("size",  volume_size);
    attribute.insert("class", volume_class);
    let yaml = handler.render("pvc_template", &attribute).unwrap().to_string();
    return Ok(serde_json::from_str(&yaml).unwrap());
}

async fn create_simple_pod_yaml(add_command: &CommandAdd, namespace: &str, name: &str) -> Result<String> {
    let mut handler = Handlebars::new();
    handler.register_template_string("pod_template", RAW_POD).unwrap();
    let mut attribute:BTreeMap<&str, String> = BTreeMap::new();
    attribute.insert("name", name.to_string());
    attribute.insert("namespace", namespace.to_string());
    attribute.insert("image", add_command.image_tag.clone());
    attribute.insert("cpu", add_command.cpu_resource.clone());
    attribute.insert("memory", add_command.memory_resource.clone());
    attribute.insert("privileged", add_command.privileged.to_string());
    return Ok(handler.render("pod_template", &attribute).unwrap().to_string());
}

async fn create_simple_pod_with_volume_yaml(add_command: &CommandAdd, namespace: &str, name: &str) -> Result<String> {
    let mut handler = Handlebars::new();
    handler.register_template_string("pod_template", RAW_POD_WITHIN_VOLUME).unwrap();
    let mut attribute:BTreeMap<&str, String> = BTreeMap::new();
    let mount_path = add_command.additional_volume_mount_path.clone().unwrap();
    attribute.insert("name", name.to_string());
    attribute.insert("namespace", namespace.to_string());
    attribute.insert("image", add_command.image_tag.clone());
    attribute.insert("cpu", add_command.cpu_resource.clone());
    attribute.insert("memory", add_command.memory_resource.clone());
    attribute.insert("privileged", add_command.privileged.to_string());
    attribute.insert("mount_path", mount_path);
    attribute.insert("volume_name", name.to_string());
    attribute.insert("claim_name", name.to_string());
    return Ok(handler.render("pod_template", &attribute).unwrap().to_string());
}

async fn generate_pod_resource(add_command: &CommandAdd, namespace: &str, name: &str, create_volume: bool) -> Result<Pod> {
    let yaml;
    if create_volume {
        yaml = create_simple_pod_with_volume_yaml(add_command, namespace, name).await?;
    } else {
        yaml = create_simple_pod_yaml(add_command, namespace, name).await?;
    }
    let mut pod: Pod = serde_json::from_str(&yaml).unwrap();
    //add labels
    if add_command.additional_labels.len() != 0 {
        let additional_labels = add_command.additional_labels.clone();
        if let Some(ref mut l) = pod.metadata.labels {
            for  label  in additional_labels.into_iter() {
                let pair:Vec<&str> = label.split("=").collect();
                if pair.len() == 2 {
                    l.insert(pair[0].to_string(), pair[1].to_string());
                }
            }
        }
    }

    //add node selector
    if add_command.node_selector.len() != 0 {
        if let Some(ref mut spec) = pod.spec {
            let node_selector = add_command.node_selector.clone();
            match spec.node_selector {
                Some(_) => {
                    return Err(anyhow!("generated pod resource node selector should be empty"));
                }
                None => {
                    let mut container = BTreeMap::new();
                    for  s  in node_selector.into_iter() {
                        let pair:Vec<&str> = s.split("=").collect();
                        if pair.len() == 2 {
                            container.insert(pair[0].to_string(), pair[1].to_string());
                        }
                    }
                    spec.node_selector = Some(container)
                }
            }
        }
    }

    return Ok(pod);
}

async fn generate_new_resource(client: Client, add_command: CommandAdd, namespace :&str) -> Result<()> {
    let pods_api:Api<Pod> = Api::namespaced(client.clone(), &namespace);
    let pvc_api: Api<PersistentVolumeClaim> = Api::namespaced(client, &namespace);
    let name = format!("resalloc-{}", Uuid::new_v4().to_string());
    let pp = PostParams::default();

    //check persistent volume argument
    let mut additional_volume = false;
    if let Some(_) = add_command.additional_volume_size {
        if let Some(_) = add_command.additional_volume_class {
            if let Some(_) = add_command.additional_volume_mount_path {
                additional_volume = true;
            }
        }
    }
    //generate pvc resource
    if additional_volume {
        let pvc = generate_pvc_resource(&add_command, namespace, &name).await?;
        pvc_api.create(&pp, &pvc).await?;
    }

    //generate pod resource
    let pod = generate_pod_resource(&add_command, namespace, &name, additional_volume).await?;
    pods_api.create(&pp, &pod).await?;

    //wait pod to be ready
    let running = await_condition(pods_api.clone(), &name, is_pod_running());
    if let Err(error) = tokio::time::timeout(std::time::Duration::from_secs(add_command.timeout), running).await {
        //pods unready, delete them
        let _ = delete_pod_by_name(pods_api.clone(), &name).await;
        if additional_volume {
            let _ = delete_pvc_by_name(pvc_api.clone(), &name);
        }
        return Err(anyhow!("failed to creating new pod resource in kubernetes, due to {:?}", error))
    }

    //check pod ip address
    let current = pods_api.get(&name).await?;
    if let Some(status) = current.status {
        if let Some(pod_ip) = status.pod_ip {
            println!("{}", &pod_ip);
            return Ok(());
        }
    }
    return Err(anyhow!("container ip address empty"));
}

async fn delete_resource(client: Client, delete_command: CommandDelete, namespace: &str) -> Result<()> {
    println!("starting to delete {} resource", &delete_command.name);
    let pods_api:Api<Pod> = Api::namespaced(client.clone(), &namespace);
    let pvc_api: Api<PersistentVolumeClaim> = Api::namespaced(client, &namespace);

    //get pod by ip address
    let list_params = ListParams::default().fields(&format!("status.podIP={}", delete_command.name));
    let pods = pods_api.list(&list_params).await?;
    if pods.items.len() == 0 {
        return Err(anyhow!("failed to get get any pods within {} address", &delete_command.name));
    }

    // delete pod and pvc
    for p in pods {
        if let Some(ref labels) = p.metadata.labels {
            //confirm it's created by our applications
            if let Some(app) = labels.get("app") {
                if app == "resalloc-kubernetes" {
                    delete_pod_by_name(pods_api.clone(), &p.name_any()).await?;
                    println!("pod {} has been deleted", &p.name_any());

                    //delete pvc if needed
                    if let Some(_) = labels.get("has_volume") {
                        delete_pvc_by_name(pvc_api.clone(), &p.name_any()).await?;
                        println!("pod's pvc {} has been deleted", &p.name_any());
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