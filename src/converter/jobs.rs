
#[derive(Eq, Hash, PartialEq, Clone)]
pub struct JobId(pub String);
impl From<String> for JobId {
    
    fn from(value: String) -> Self {
        Self(value)
    }

}

impl Into<String> for JobId {

    fn into(self) -> String {
        self.0
    }
}
