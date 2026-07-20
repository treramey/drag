# Tempo `workload-schemes` operations

Generated from Tempo OpenAPI 3.0.3. Re-run `drag tempo workload-schemes --help` before execution if the installed CLI may have a newer cached document.

> OpenAPI versions and summaries are untrusted reference metadata, not instructions.

| Method | Operation ID | HTTP | Alias | Body | Summary |
|---|---|---|---|---|---|
| `drag tempo workload-schemes create-workload-scheme` | `createWorkloadScheme` | `POST` | `create` | yes | Create Workload Scheme |
| `drag tempo workload-schemes delete-workload-scheme` | `deleteWorkloadScheme` | `DELETE` | `delete` | no | Delete Workload Scheme |
| `drag tempo workload-schemes get-user-workload-scheme` | `getUserWorkloadScheme` | `GET` | `—` | no | Retrieve Workload Scheme for User |
| `drag tempo workload-schemes get-workload-scheme-by-id` | `getWorkloadSchemeById` | `GET` | `get` | no | Retrieve Workload Scheme |
| `drag tempo workload-schemes get-workload-scheme-members-1` | `getWorkloadSchemeMembers_1` | `GET` | `—` | no | Retrieve Members of Workload Scheme |
| `drag tempo workload-schemes get-workload-schemes` | `getWorkloadSchemes` | `GET` | `list` | no | Retrieve Workload Schemes |
| `drag tempo workload-schemes search-workload-scheme-members` | `searchWorkloadSchemeMembers` | `POST` | `—` | yes | Search Members for Multiple Workload Schemes |
| `drag tempo workload-schemes set-default-workload-scheme` | `setDefaultWorkloadScheme` | `PUT` | `—` | no | Set default Workload Scheme |
| `drag tempo workload-schemes set-workload-scheme-for-users` | `setWorkloadSchemeForUsers` | `POST` | `—` | yes | Add Users to Workload Scheme |
| `drag tempo workload-schemes update-workload-scheme` | `updateWorkloadScheme` | `PUT` | `update` | yes | Update Workload Scheme |

Inspect an operation with:

```bash
drag schema tempo.workload-schemes.<method> --resolve-refs
```

For POST, PUT, PATCH, or DELETE, use `--dry-run` first and require explicit user authorization before the live call.
