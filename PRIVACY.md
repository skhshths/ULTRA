# ULTRA Privacy Policy

*Effective [DATE]. Applies to the ULTRA web browser for Windows and macOS and to ULTRA chat.*

ULTRA does not collect, sell or share your browsing data. Your history, bookmarks, tabs and
settings stay on your device. The only data that leaves it is what is needed to check for
updates, block unsafe sites, and deliver chat messages, as described below.

## 1. Who we are

ULTRA is made by [NAME] ("we", "us"). Questions or requests: [CONTACT EMAIL].

## 2. Data that stays on your device

ULTRA saves the following in a folder on your computer (`%LOCALAPPDATA%\Ultra` on Windows,
`~/Library/Application Support/Ultra` on macOS). We never receive it.

- Browsing history, bookmarks, shortcuts, open tabs and layout, downloads list and settings.
- Cookies, site data and sign-ins, kept by the browser engine (Microsoft WebView2 or Apple WebKit).
- Your chat identity key, your chats, and your contacts' keys. The private key is encrypted with
  your operating-system account where available.
- Crash reports (`crash.log`). These are only sent if you choose to share one with us.

ULTRA does not save passwords. Deleting the folder removes all of this data. Private-workspace
browsing is not saved to history or sessions.

## 3. Data that leaves your device

| Service | When | What is sent | Who receives it |
| --- | --- | --- | --- |
| Update checks | When ULTRA starts and every 4 hours (can be turned off in settings) | A standard web request for the latest version, including your IP address | GitHub (Microsoft) |
| Safe-browsing checks (Windows) | When you visit sites or download files | Site addresses, checked against lists of phishing and malware sites | Microsoft Defender SmartScreen, under [Microsoft's privacy statement](https://privacy.microsoft.com/privacystatement) |
| Search | When you search from the address bar | Your search terms | The search engine you choose |
| Ad-block list updates | About once a week, in versions that refresh their lists | A standard web request for the filter lists | The list publishers (EasyList, uBlock Origin, Peter Lowe) |
| ULTRA chat | Only if you create an ULTRA ID | See section 4 | Our chat relay, through Cloudflare |

Websites you visit receive what any browser sends them, such as your IP address. Their own
privacy policies apply to them.

## 4. ULTRA chat

Chat is optional. If you create an ULTRA ID, our relay stores:

- your @handle, display name and public key, and when you joined and were last seen;
- your friend list and pending friend requests;
- messages waiting to be delivered.

Messages are end-to-end encrypted on your device before they are sent. We cannot read them.
Each message is deleted from the relay as soon as it is delivered.

To prevent abuse, the relay uses your IP address to limit how many new handles can be created
from one connection. It keeps the address in memory only, for up to one day, and does not write
it to disk or to logs. Traffic passes through Cloudflare, which processes it under
[Cloudflare's privacy policy](https://www.cloudflare.com/privacypolicy/).

## 5. What we don't do

No analytics, telemetry, advertising IDs, tracking pixels or data sales. We have no ULTRA
accounts beyond the optional chat ID.

## 6. Your choices and rights

- Turn off automatic updates in settings.
- Clear history and site data in settings, or delete the data folder.
- Delete your ULTRA ID in settings (**ULTRA ID → delete**). This removes your handle, friend
  list, friend requests and waiting messages from the relay at once, then your ID and chats from
  your computer. If you can no longer sign in, email [CONTACT EMAIL] with your @handle.
- Depending on where you live (for example under the GDPR or California law), you may have the
  right to access, correct or delete data about you. Contact us and we will respond within
  30 days.

## 7. Children

ULTRA chat is not for children under 13, and we do not knowingly hold data from them. If you
believe a child under 13 has created an ULTRA ID, contact us and we will delete it.

## 8. Security

Local secrets are encrypted with your operating-system account where available. Chat messages
are end-to-end encrypted, and updates are verified against published checksums. No system is
perfectly secure, and we cannot guarantee that data will never be exposed.

## 9. Changes

We will post changes to this policy in ULTRA's GitHub repository and update the effective date.
Significant changes will be noted in the release notes.
