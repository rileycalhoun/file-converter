// @generated automatically by Diesel CLI.

diesel::table! {
    files (id) {
        id -> Int4,
        file_name -> Varchar,
        content -> Text,
    }
}
