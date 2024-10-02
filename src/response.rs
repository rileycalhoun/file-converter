use serde::Deserialize;

#[derive(Deserialize)]
pub struct CreateResponse {
    pub id: String
}

#[derive(Deserialize)]
pub struct Job {
    pub id: String,
    pub tasks: Vec<JobTask>
}

#[derive(Deserialize)]
pub struct JobTask {
    pub id: String,
    pub operation: String,
    pub result: TaskResult
}

#[derive(Deserialize)]
pub struct TaskResult {
    pub files: Vec<TaskFile>
}

#[derive(Deserialize)]
pub struct TaskFile {
    #[serde(rename = "filename")]
    pub file_name: String,

    pub url: Option<String>
}
