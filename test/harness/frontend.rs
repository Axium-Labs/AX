use super::*;

#[tokio::test]
async fn cli_and_acp_automatically_dispatch_twenty_three_typed_discovered_items() {
    for surface in [Surface::Cli, Surface::Acp] {
        let mut fixture = Fixture::new(23, false, false);
        let tasks=(1..=23).map(|id|json!({"title":format!("instance {id}"),"input":format!("Process instance {id} from instances.json. Produce a terminal outcome.")})).collect::<Vec<_>>();
        fs::write(
            &fixture.provider.dataset,
            json!({"ax_work_items":tasks}).to_string(),
        )
        .unwrap();
        fixture
            .run(
                surface,
                "Read instances.json and execute its concrete work inventory",
            )
            .await;
        fixture.check_children(23);
        fixture.cleanup();
    }
}
