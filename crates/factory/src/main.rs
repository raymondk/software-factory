use api_client::{Client, CreateTicket, Ticket};
use clap::{Parser, Subcommand};

/// CLI for the Software Factory orchestrator.
#[derive(Parser)]
struct Cli {
    /// Orchestrator URL
    #[arg(long, env = "FACTORY_URL", global = true, default_value = "http://localhost:8080")]
    url: String,
    /// Bearer token
    #[arg(long, env = "FACTORY_TOKEN", global = true, hide_env_values = true)]
    token: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Work with tickets
    Ticket {
        #[command(subcommand)]
        command: TicketCommand,
    },
}

#[derive(Subcommand)]
enum TicketCommand {
    /// Create a ticket
    Create {
        #[arg(long)]
        title: String,
        #[arg(long, default_value = "")]
        description: String,
    },
    /// List tickets in rank order
    List,
    /// Show one ticket
    View { id: i64 },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let token = cli.token.ok_or_else(|| anyhow::anyhow!("set FACTORY_TOKEN or pass --token"))?;
    let client = Client::new(cli.url, token);
    match cli.command {
        Command::Ticket { command } => match command {
            TicketCommand::Create { title, description } => {
                print(&client.create_ticket(&CreateTicket { title, description }).await?)
            }
            TicketCommand::List => {
                for t in client.list_tickets().await? {
                    println!("#{}\t{}\t{}\t{}", t.id, t.state, t.rank, t.title);
                }
            }
            TicketCommand::View { id } => print(&client.get_ticket(id).await?),
        },
    }
    Ok(())
}

fn print(t: &Ticket) {
    println!("{}", serde_json::to_string_pretty(t).unwrap());
}
