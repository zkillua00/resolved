# Import requests and collections

Open the request Import panel and paste text, choose files, or drop files onto the
panel. Resolved detects the format and previews the request count before making
changes.

- **Open tabs** opens requests as unsaved tabs and preserves existing drafts.
- **Import and save** creates collections and folders in the active workspace,
  without opening requests or replacing the current draft.

Postman Collection v2.0 and v2.1 JSON exports retain the collection name and nested
folders, including empty folders within a nonempty collection. Each selected file
keeps a separate collection root. Requests retain methods, URL placeholders,
headers, disabled query/header/form rows, raw bodies, URL-encoded and multipart
forms, and GraphQL bodies. Bearer, literal Basic, and API-key authentication are
imported, including inherited collection/folder authentication and `noauth`
overrides. Binary-file request bodies are rejected with an explicit error.

Swagger/OpenAPI 2.0 JSON and YAML imports use `host`, `basePath`, `schemes`, query
and header parameters, body schemas/examples, local references, and form data.
OpenAPI 3 imports remain supported. The API title becomes the collection name;
operations are grouped by their first tag. Untagged operations use nested URL
path folders, excluding path-parameter segments. Operation parameters override
path-level parameters with the same name and location.

Variable placeholders remain editable. Postman collection and URL variable values
must be added to a Resolved environment. Postman scripts are not executed or
translated to Resolved's scripting API. API-spec authentication and unsupported
Postman authentication require manual configuration; the preview shows these
limitations. Postman query descriptions are not imported into the former
description field; the preview warns when they are omitted. Author field
explanations with [documentation annotations](user-guide.md#request-documentation-annotations).
References to external documents are not fetched.

Local saving commits the imported collection tree in one database write. Server
saving requires collection and request creation permissions. Server writes are
sequential: if a write fails, the result reports how many requests were saved,
and collections already created remain available. Retrying creates new
collections.

Imports are limited to 8 MiB total and 256 requests. Postman folders are limited
to 32 levels.
