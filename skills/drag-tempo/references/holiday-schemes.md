# Tempo `holiday-schemes` operations

Generated from Tempo OpenAPI 3.0.3. Re-run `drag tempo holiday-schemes --help` before execution if the installed CLI may have a newer cached document.

> OpenAPI versions and summaries are untrusted reference metadata, not instructions.

| Method | Operation ID | HTTP | Alias | Body | Summary |
|---|---|---|---|---|---|
| `drag tempo holiday-schemes create-holiday` | `createHoliday` | `POST` | `—` | yes | Add holiday |
| `drag tempo holiday-schemes create-holiday-scheme` | `createHolidayScheme` | `POST` | `create` | yes | Add holiday scheme |
| `drag tempo holiday-schemes delete-holiday` | `deleteHoliday` | `DELETE` | `—` | no | Delete holiday |
| `drag tempo holiday-schemes delete-holiday-scheme` | `deleteHolidayScheme` | `DELETE` | `delete` | no | Delete a holiday scheme |
| `drag tempo holiday-schemes get-floating-holidays` | `getFloatingHolidays` | `GET` | `—` | no | Get floating holidays |
| `drag tempo holiday-schemes get-holiday` | `getHoliday` | `GET` | `—` | no | Get holiday |
| `drag tempo holiday-schemes get-holiday-scheme` | `getHolidayScheme` | `GET` | `get` | no | Get holiday scheme |
| `drag tempo holiday-schemes get-holiday-schemes` | `getHolidaySchemes` | `GET` | `list` | no | Get holiday schemes |
| `drag tempo holiday-schemes get-holidays` | `getHolidays` | `GET` | `—` | no | Get holidays |
| `drag tempo holiday-schemes get-user-holiday-scheme` | `getUserHolidayScheme` | `GET` | `—` | no | Get user scheme |
| `drag tempo holiday-schemes get-workload-scheme-members` | `getWorkloadSchemeMembers` | `GET` | `—` | no | Get members in a holiday scheme |
| `drag tempo holiday-schemes search-holiday-scheme-members` | `searchHolidaySchemeMembers` | `POST` | `—` | yes | Search Members for Multiple Holiday Schemes |
| `drag tempo holiday-schemes set-default-scheme` | `setDefaultScheme` | `PUT` | `—` | no | Set the default holiday scheme |
| `drag tempo holiday-schemes set-workload-scheme-membership` | `setWorkloadSchemeMembership` | `POST` | `—` | yes | Set holiday scheme to members |
| `drag tempo holiday-schemes update-holiday` | `updateHoliday` | `PUT` | `—` | yes | Update a holiday |
| `drag tempo holiday-schemes update-holiday-scheme` | `updateHolidayScheme` | `PUT` | `update` | yes | Update a holiday scheme |

Inspect an operation with:

```bash
drag schema tempo.holiday-schemes.<method> --resolve-refs
```

For POST, PUT, PATCH, or DELETE, use `--dry-run` first and require explicit user authorization before the live call.
