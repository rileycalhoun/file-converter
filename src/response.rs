use serde::{Serialize, Deserialize};

#[derive(Serialize)]
pub struct AppResponse {
    pub ok: bool
}

#[derive(Deserialize)]
pub struct CreateResponse {
    pub data: CreateResponseData
}

#[derive(Deserialize)]
pub struct CreateResponseData {
    pub id: String
}

#[derive(Deserialize)]
pub struct JobResponse {
    pub event: String,
    pub job: Job
}

#[derive(Deserialize)]
pub struct Job {
    pub id: String,
    pub tag: String,
    pub tasks: Box<[JobTask]>
}

#[derive(Deserialize)]
pub struct JobTask {
    pub id: String,
    pub name: String,
    pub operation: String,
    pub percent: i8,
    pub result: Box<[TaskFile]>
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TaskFile {
    #[serde(rename = "filename")]
    pub file_name: String,
    pub url: String
}
