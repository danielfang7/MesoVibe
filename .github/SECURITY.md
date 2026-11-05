# Security policy

## Reporting a vulnerability

Please report security issues privately through GitHub: open the repository's **Security** tab and choose **Report a vulnerability**. Don't open a public issue for them.

Include what you found, how to reproduce it, and which version of MesoVibe you used (Settings > About). You can expect a reply within a week.

## Supported versions

Only the latest release is supported. MesoVibe updates itself, so fixes ship as a new release.

## Scope

Of particular interest:

- the updater: anything that would let an installed copy accept an update that wasn't signed with the project's update key;
- the panel and plugins: anything that lets text read from the game, or a plugin, run script or reach the host beyond the plugin API;
- anything that makes MesoVibe send data off the computer other than its daily update check.
