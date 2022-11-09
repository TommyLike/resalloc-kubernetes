use std::collections::BTreeMap;
use k8s_openapi::api::core::v1::Pod;
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
    #[arg(long, default_value_t = 60)]
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
    let pods_api:Api<Pod> = Api::namespaced(client, &namespace);
    //log preparation
    //handle kubernetes pod resource
    match app.command {
        Some(Commands::Add(add_command)) => {
            generate_new_pod(add_command, &namespace, pods_api).await.unwrap();
        }
        Some(Commands::Delete(delete_command)) => {
            delete_pod(delete_command, pods_api,).await.unwrap();
        }
        None => {
        }
    };
    Ok(())
}

async fn generate_pod_yaml(add_command: &CommandAdd, namespace: &str, name: &str) -> Result<String> {
    let mut handler = Handlebars::new();
    handler.register_template_string("pod_template", RAW_POD).unwrap();
    let mut attribute:BTreeMap<&str, &str> = BTreeMap::new();
    let privileged = add_command.privileged.to_string();
    attribute.insert("name", name);
    attribute.insert("namespace", namespace);
    attribute.insert("image", &add_command.image_tag);
    attribute.insert("cpu", &add_command.cpu_resource);
    attribute.insert("memory", &add_command.memory_resource);
    attribute.insert("privileged", &privileged);
    return Ok(handler.render("pod_template", &attribute).unwrap().to_string());
}

async fn generate_new_pod(add_command: CommandAdd, namespace :&str, pods_api: Api<Pod>) -> Result<()> {
    //generate pod yaml
    let name = format!("resalloc-{}", Uuid::new_v4().to_string());
    let yaml = generate_pod_yaml(&add_command, namespace, &name).await?;
    //todo: support persistent volume, node selector and additional labels
    println!("{}", yaml);
    let pod: Pod = serde_json::from_str(&yaml).unwrap();
    let pp = PostParams::default();
    pods_api.create(&pp, &pod).await.unwrap();
    //wait pod to be ready
    let running = await_condition(pods_api.clone(), &name, is_pod_running());
    let _ = tokio::time::timeout(std::time::Duration::from_secs(add_command.timeout), running).await?;
    //check pod ip address
    let current = pods_api.get(&name).await?;
    if let Some(status) = current.status {
        if let Some(pod_ip) = status.pod_ip {
            println!("{}", &pod_ip)
        }
    }
    Ok(())
}

async fn delete_pod(delete_command: CommandDelete, pods_api: Api<Pod>) -> Result<()> {
    println!("starting to delete {} resource", &delete_command.name);
    let list_params = ListParams::default().fields(&format!("status.podIP={}", delete_command.name));
    let pods = pods_api.list(&list_params).await?;
    if pods.items.len() == 0 {
        return Err(anyhow!("failed to get get any pods within {} address", &delete_command.name));
    }
    for p in pods {
        let delete_params = DeleteParams::default();
        pods_api.delete(&p.name_any(), &delete_params).await?;
        println!("pod {} has been deleted", &p.name_any());
    }
    Ok(())
}