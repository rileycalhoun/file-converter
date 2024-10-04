use web_push_native::WebPushBuilder;


pub async fn push(message: serde_json::Value, builder: WebPushBuilder) -> Result<(), Box<dyn std::error::Error>> {
    Ok(())
}
