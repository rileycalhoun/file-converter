use diesel::prelude::*;

#[derive(Queryable, Selectable)]
#[diesel(table_name = crate::database::schema::files)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct File {
    pub id: i32,
    pub file_name: String,
    pub content: String
}

#[derive(Insertable)]
#[diesel(table_name = crate::database::schema::files)]
pub struct NewFile<'de> {
    pub file_name: &'de str,
    pub content: &'de str
}
