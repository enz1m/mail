pub(super) fn key(source: &str) -> Option<&'static str> {
    match source {
        "Notifications" => Some("mail-notifications-title"),
        "Could not save notification settings" => Some("mail-notifications-save-failed"),
        "Done" => Some("mail-done"),
        "The message mentions an attachment, but no file is attached." => {
            Some("mail-attachment-missing")
        }
        "All accounts" => Some("mail-all-accounts"),
        "Remove account?" => Some("mail-remove-account-question"),
        "Open in the full composer" => Some("mail-open-full-composer"),
        "Quick reply" => Some("mail-quick-reply"),
        "Search mail…" => Some("mail-search-mail"),
        "No matching messages" => Some("mail-no-matching-messages"),
        "Local mail state is read-only until it is repaired." => Some("mail-local-read-only"),
        "The unreadable file was kept as a recovery copy, so this session starts with no local mail." => {
            Some("mail-unreadable-recovery")
        }
        "value" => Some("mail-rule-value"),
        "Remote image blocked" => Some("mail-remote-image-blocked"),
        "Loading your saved mail, preferences and shortcuts…" => Some("mail-loading-saved-data"),
        "Attachment actions" => Some("mail-attachment-actions"),
        "Empty" => Some("mail-empty"),
        "Empty Mailbox" => Some("mail-empty-mailbox"),
        "Empty Junk" => Some("mail-empty-junk"),
        "Empty Trash" => Some("mail-empty-trash"),
        "Remove from Smart Mailboxes" => Some("mail-remove-from-smart-mailboxes"),
        "Remove from Favorites" => Some("mail-remove-from-favorites"),
        "Add to Favorites" => Some("mail-add-to-favorites"),
        "Mark All as Read" => Some("mail-mark-all-read"),
        "e.g.\n\nDanny\nHost of the Sunday lunches" => Some("mail-signature-example"),
        "Could not read the local clock. Try again." => Some("mail-clock-unavailable"),
        "Choose a date and time in the future." => Some("mail-choose-future-time"),
        "Not yet in effect — saved for when read-receipt handling is available." => {
            Some("mail-receipts-not-supported")
        }
        "Mail has no browser sign-in for this server. Choose Password under Advanced." => {
            Some("mail-no-browser-sign-in")
        }
        "SMTP (Outgoing) — optional for this protocol" => Some("mail-smtp-optional"),
        "Blocked Sender" => Some("mail-blocked-sender"),
        "The mail worker stopped unexpectedly." => Some("mail-transport-error-17"),
        "The mail worker is unavailable." => Some("mail-transport-error-16"),
        "The mail operation could not be started." => Some("mail-transport-error-15"),
        "A secure connection to the mail server could not be established." => {
            Some("mail-transport-error-14")
        }
        "The mail server did not respond before the timeout." => Some("mail-transport-error-13"),
        "The mail server response exceeded the safety limit." => Some("mail-transport-error-12"),
        "The mail server returned an invalid response." => Some("mail-transport-error-11"),
        "This build cannot safely perform the requested mail operation." => {
            Some("mail-transport-error-10")
        }
        "The mail server could not be reached." => Some("mail-transport-error-9"),
        "The outgoing message exceeds the safety limit." => Some("mail-transport-error-8"),
        "The server mutation outcome is unknown. Its durable identity was kept for reconciliation before retry." => {
            Some("mail-transport-error-6")
        }
        "Server acceptance is unknown. The Outbox item is kept with the same Message-ID for an explicit Retry." => {
            Some("mail-transport-error-5")
        }
        "The mail server settings are not supported." => Some("mail-transport-error-4"),
        "The mail operation was cancelled." => Some("mail-transport-error-3"),
        "Another mail operation is running. Try again in a moment." => {
            Some("mail-transport-error-2")
        }
        "The mail server rejected the account credentials." => Some("mail-transport-error-1"),
        "A fetched mailbox carried a stale synchronization generation." => {
            Some("mail-a-fetched-mailbox-carried-a-stale-synchronization-generation")
        }
        "A file already exists at that location. Choose a new destination." => {
            Some("mail-a-file-already-exists-at-that-location-choose-a-new-destination")
        }
        "A mail service operation did not stop before the close deadline." => {
            Some("mail-a-mail-service-operation-did-not-stop-before-the-close-deadline")
        }
        "A safe attachment identity could not be created." => {
            Some("mail-a-safe-attachment-identity-could-not-be-created")
        }
        "Account not added" => Some("mail-account-not-added"),
        "Account not removed" => Some("mail-account-not-removed"),
        "Account removal stopped" => Some("mail-account-removal-stopped"),
        "Account removed" => Some("mail-account-removed"),
        "Account setup changed before it could be saved. Cancel and add the account again." => {
            Some(
                "mail-account-setup-changed-before-it-could-be-saved-cancel-and-add-the-account-again",
            )
        }
        "Account update completion ignored" => Some("mail-account-update-completion-ignored"),
        "Account updated" => Some("mail-account-updated"),
        "Accounts" => Some("mail-accounts"),
        "Actions" => Some("mail-actions"),
        "Adapt HTML emails to dark mode" => Some("mail-adapt-html-emails-to-dark-mode"),
        "Add Account" => Some("mail-add-account"),
        "Add Account…" => Some("mail-add-account-prompt"),
        "Add CC or BCC" => Some("mail-add-cc-or-bcc"),
        "Add a smart mailbox" => Some("mail-add-a-smart-mailbox"),
        "Add action" => Some("mail-add-action"),
        "Add condition" => Some("mail-add-condition"),
        "Add your email account to read and send mail here. Most accounts need only your name, address and password." => {
            Some(
                "mail-add-your-email-account-to-read-and-send-mail-here-most-accounts-need-only-your-name-address-and-password",
            )
        }
        "Advanced" => Some("mail-advanced"),
        "All" => Some("mail-all"),
        "All Mailboxes" => Some("mail-all-mailboxes"),
        "Allow by default" => Some("mail-allow-by-default"),
        "Allow once" => Some("mail-allow-once"),
        "Always allow" => Some("mail-always-allow"),
        "Always block" => Some("mail-always-block"),
        "Always block (strip images)" => Some("mail-always-block-strip-images"),
        "Append signature to new messages" => Some("mail-append-signature-to-new-messages"),
        "Apply rules" => Some("mail-apply-rules"),
        "Archive" => Some("mail-archive"),
        "Archive selected" => Some("mail-archive-selected"),
        "Ask per sender" => Some("mail-ask-per-sender"),
        "Ask per sender (recommended)" => Some("mail-ask-per-sender-recommended"),
        "Attach a file" => Some("mail-attach-a-file"),
        "Attach file" => Some("mail-attach-file"),
        "Attachment bytes were published, but private-store durability is uncertain." => {
            Some("mail-attachment-bytes-were-published-but-private-store-durability-is-uncertain")
        }
        "Attachment data unavailable — metadata only" => {
            Some("mail-attachment-data-unavailable-metadata-only")
        }
        "Attachment details" => Some("mail-attachment-details"),
        "Attachment handed to the selected application." => {
            Some("mail-attachment-handed-to-the-selected-application")
        }
        "Attachment maintenance is incomplete. No new attachment bytes can be stored yet." => Some(
            "mail-attachment-maintenance-is-incomplete-no-new-attachment-bytes-can-be-stored-yet",
        ),
        "Attachment saved." => Some("mail-attachment-saved"),
        "Attachment work could not start yet and will be retried." => {
            Some("mail-attachment-work-could-not-start-yet-and-will-be-retried")
        }
        "Attachment work was cancelled before it completed." => {
            Some("mail-attachment-work-was-cancelled-before-it-completed")
        }
        "Attachments" => Some("mail-attachments"),
        "Authentication" => Some("mail-authentication"),
        "Automatic colour" => Some("mail-automatic-colour"),
        "Automatically organize incoming mail" => Some("mail-automatically-organize-incoming-mail"),
        "BCC" => Some("mail-bcc"),
        "Back" => Some("mail-back"),
        "Back to message list" => Some("mail-back-to-message-list"),
        "Background work is unavailable until Mail restarts." => {
            Some("mail-background-work-is-unavailable-until-mail-restarts")
        }
        "Background work is unavailable until the application restarts." => {
            Some("mail-background-work-is-unavailable-until-the-application-restarts")
        }
        "Background-work identity was exhausted; restart is required." => {
            Some("mail-background-work-identity-was-exhausted-restart-is-required")
        }
        "Behaviors" => Some("mail-behaviors"),
        "Block Sender" => Some("mail-block-sender"),
        "Block sender?" => Some("mail-block-sender-prompt"),
        "Blue text" => Some("mail-blue-text"),
        "CC" => Some("mail-cc"),
        "Cancel" => Some("mail-cancel"),
        "Cancel sign-in" => Some("mail-cancel-sign-in"),
        "Change send time" => Some("mail-change-send-time"),
        "Change time" => Some("mail-change-time"),
        "Choose a conversation from the list to read it here." => {
            Some("mail-choose-a-conversation-from-the-list-to-read-it-here")
        }
        "Choose one conversation to read it, or use the actions above." => {
            Some("mail-choose-one-conversation-to-read-it-or-use-the-actions-above")
        }
        "Clear flag" => Some("mail-clear-flag"),
        "Clear selection" => Some("mail-clear-selection"),
        "Close composer" => Some("mail-close-composer"),
        "Close without saving" => Some("mail-close-without-saving"),
        "Compose" => Some("mail-compose"),
        "Compose message" => Some("mail-compose-message"),
        "Compose to recipient" => Some("mail-compose-to-recipient"),
        "Filter messages from recipient" => Some("mail-filter-messages-from-recipient"),
        "Compose to sender" => Some("mail-compose-to-sender"),
        "Conditions" => Some("mail-conditions"),
        "Configuration saved; credential cleanup pending" => {
            Some("mail-configuration-saved-credential-cleanup-pending")
        }
        "Configuration saved; credential reconciliation pending" => {
            Some("mail-configuration-saved-credential-reconciliation-pending")
        }
        "Confirming the password-only credential update…" => {
            Some("mail-confirming-the-password-only-credential-update")
        }
        "Continue" => Some("mail-continue"),
        "Conversation actions" => Some("mail-conversation-actions"),
        "Copy" => Some("mail-copy"),
        "Copy address" => Some("mail-copy-address"),
        "Copy hidden from the others" => Some("mail-copy-hidden-from-the-others"),
        "Copy link address" => Some("mail-copy-link-address"),
        "Copy name and email" => Some("mail-copy-name-and-email"),
        "Copy, seen by everyone" => Some("mail-copy-seen-by-everyone"),
        "Could not finish saving before close" => Some("mail-could-not-finish-saving-before-close"),
        "Could not remove account credential" => Some("mail-could-not-remove-account-credential"),
        "Could not safely close" => Some("mail-could-not-safely-close"),
        "Could not save privacy settings" => Some("mail-could-not-save-privacy-settings"),
        "Could not save reading settings" => Some("mail-could-not-save-reading-settings"),
        "Could not save rules" => Some("mail-could-not-save-rules"),
        "Could not save sender permissions" => Some("mail-could-not-save-sender-permissions"),
        "Could not save sidebar settings" => Some("mail-could-not-save-sidebar-settings"),
        "Could not save the message format" => Some("mail-could-not-save-the-message-format"),
        "Credential cleanup incomplete" => Some("mail-credential-cleanup-incomplete"),
        "Credential cleanup pending" => Some("mail-credential-cleanup-pending"),
        "Credential reconciliation pending" => Some("mail-credential-reconciliation-pending"),
        "Credential state reconciled" => Some("mail-credential-state-reconciled"),
        "Credential state uncertain" => Some("mail-credential-state-uncertain"),
        "Custom" => Some("mail-custom"),
        "Date" => Some("mail-date"),
        "Default message display" => Some("mail-default-message-display"),
        "Default shortcuts are in use. Saved shortcut data is protected and will not be overwritten." => {
            Some(
                "mail-default-shortcuts-are-in-use-saved-shortcut-data-is-protected-and-will-not-be-overwritten",
            )
        }
        "Delete" => Some("mail-delete"),
        "Delete (move to Trash)" => Some("mail-delete-move-to-trash"),
        "Delete rule" => Some("mail-delete-rule"),
        "Delete selected (move to Trash)" => Some("mail-delete-selected-move-to-trash"),
        "Delivery not confirmed" => Some("mail-delivery-not-confirmed"),
        "Delivery waits until the undo period ends unless you choose Send now. If submission fails, the message remains in the durable Outbox for Retry or Cancel." => {
            Some(
                "mail-delivery-waits-until-the-undo-period-ends-if-submission-fails-the-message-remains-in-the-durable-outbox-for-retry-or-cancel",
            )
        }
        "Delivery will start after the queued message is durable." => {
            Some("mail-delivery-will-start-after-the-queued-message-is-durable")
        }
        "Destination" => Some("mail-destination"),
        "Discard" => Some("mail-discard"),
        "Discard inline reply" => Some("mail-discard-inline-reply"),
        "Dismiss attachment reminder" => Some("mail-dismiss-attachment-reminder"),
        "Draft not saved" => Some("mail-draft-not-saved"),
        "Draft saved locally" => Some("mail-draft-saved-locally"),
        "Drafts" => Some("mail-drafts"),
        "Edit Account" => Some("mail-edit-account"),
        "Edit Draft" => Some("mail-edit-draft"),
        "Edit rule" => Some("mail-edit-rule"),
        "Email links can hide their real destination. Open this address in your default application?" => {
            Some(
                "mail-email-links-can-hide-their-real-destination-open-this-address-in-your-default-application",
            )
        }
        "Enter an account password." => Some("mail-enter-an-account-password"),
        "Enter your email address" => Some("mail-enter-your-email-address"),
        "Exchange" => Some("mail-exchange"),
        "Family Pictures" => Some("mail-family-pictures"),
        "Favorites" => Some("mail-favorites"),
        "Filter" => Some("mail-filter"),
        "Filter messages from sender" => Some("mail-filter-messages-from-sender"),
        "Flag" => Some("mail-flag"),
        "Flag / unflag" => Some("mail-flag-unflag"),
        "Flagged" => Some("mail-flagged"),
        "Forward" => Some("mail-forward"),
        "From" => Some("mail-from"),
        "General" => Some("mail-general"),
        "Green text" => Some("mail-green-text"),
        "HTML" => Some("mail-html"),
        "HTML display unavailable. Showing readable text." => {
            Some("mail-html-display-unavailable-showing-readable-text")
        }
        "HTML message" => Some("mail-html-message"),
        "HTML-only message" => Some("mail-html-only-message"),
        "Help" => Some("mail-help"),
        "Hide my IP address (privacy proxy)" => Some("mail-hide-my-ip-address-privacy-proxy"),
        "IMAP" => Some("mail-imap"),
        "IMAP (Incoming)" => Some("mail-imap-incoming"),
        "IP privacy and read receipts" => Some("mail-ip-privacy-and-read-receipts"),
        "Images blocked" => Some("mail-images-blocked"),
        "In 3 hours" => Some("mail-in-3-hours"),
        "Inbox" => Some("mail-inbox"),
        "Incoming" => Some("mail-incoming"),
        "Incoming port" => Some("mail-incoming-port"),
        "Inline image bundled with the message" => {
            Some("mail-inline-image-bundled-with-the-message")
        }
        "JMAP" => Some("mail-jmap"),
        "Junk" => Some("mail-junk"),
        "Keep as draft" => Some("mail-keep-as-draft"),
        "Keep window open" => Some("mail-keep-window-open"),
        "Keyboard Shortcuts" => Some("mail-keyboard-shortcuts"),
        "Keyboard shortcuts" => Some("mail-keyboard-shortcuts-label"),
        "Leave blank to keep the current password" => {
            Some("mail-leave-blank-to-keep-the-current-password")
        }
        "Load older messages" => Some("mail-load-older-messages"),
        "Loading message…" => Some("mail-loading-message"),
        "Local payload reference" => Some("mail-local-payload-reference"),
        "Local time. Mail must be running and connected to send; otherwise it sends when available." => {
            Some(
                "mail-local-time-mail-must-be-running-and-connected-to-send-otherwise-it-sends-when-available",
            )
        }
        "Login" => Some("mail-login"),
        "Mail" => Some("mail-mail"),
        "Mail Settings" => Some("mail-mail-settings"),
        "Mail completion ignored" => Some("mail-mail-completion-ignored"),
        "Mail looks up your provider's server settings from your address." => {
            Some("mail-mail-looks-up-your-provider-s-server-settings-from-your-address")
        }
        "Mail operation queue exhausted" => Some("mail-mail-operation-queue-exhausted"),
        "Mail protocol mismatch" => Some("mail-mail-protocol-mismatch"),
        "Mailbox actions" => Some("mail-mailbox-actions"),
        "Mailbox content ignored" => Some("mail-mailbox-content-ignored"),
        "Mailbox content was not staged" => Some("mail-mailbox-content-was-not-staged"),
        "Mailbox identity changed" => Some("mail-mailbox-identity-changed"),
        "Mailbox not found on server" => Some("mail-mailbox-not-found-on-server"),
        "Mailbox refresh unavailable" => Some("mail-mailbox-refresh-unavailable"),
        "Mailbox synchronization incomplete" => Some("mail-mailbox-synchronization-incomplete"),
        "Mailbox synchronization needs restart" => {
            Some("mail-mailbox-synchronization-needs-restart")
        }
        "Mailbox synchronization needs retry" => Some("mail-mailbox-synchronization-needs-retry"),
        "Mailbox synchronization not adopted" => Some("mail-mailbox-synchronization-not-adopted"),
        "Mailbox synchronization not saved" => Some("mail-mailbox-synchronization-not-saved"),
        "Mailbox synchronization paused" => Some("mail-mailbox-synchronization-paused"),
        "Mailbox synchronization waiting to save" => {
            Some("mail-mailbox-synchronization-waiting-to-save")
        }
        "Mailboxes" => Some("mail-mailboxes"),
        "Mark as Read" => Some("mail-mark-as-read"),
        "Mark as Unread" => Some("mail-mark-as-unread"),
        "Mark as read" => Some("mail-mark-as-read-label"),
        "Mark as unread" => Some("mail-mark-as-unread-label"),
        "Mark read / unread" => Some("mail-mark-read-unread"),
        "Message" => Some("mail-message"),
        "Message accepted; local copy pending" => Some("mail-message-accepted-local-copy-pending"),
        "Message remains in Outbox" => Some("mail-message-remains-in-outbox"),
        "Message sent" => Some("mail-message-sent"),
        "Move" => Some("mail-move"),
        "Move down" => Some("mail-move-down"),
        "Move to Junk" => Some("mail-move-to-junk"),
        "Move up" => Some("mail-move-up"),
        "Navigation" => Some("mail-navigation"),
        "Never automatically send read receipts" => {
            Some("mail-never-automatically-send-read-receipts")
        }
        "Never block" => Some("mail-never-block"),
        "New" => Some("mail-new"),
        "New Message" => Some("mail-new-message"),
        "New message" => Some("mail-new-message-label"),
        "New password" => Some("mail-new-password"),
        "New rule" => Some("mail-new-rule"),
        "Next conversation" => Some("mail-next-conversation"),
        "Next week" => Some("mail-next-week"),
        "No account selected." => Some("mail-no-account-selected"),
        "No accounts yet. Add one above to start receiving mail." => {
            Some("mail-no-accounts-yet-add-one-above-to-start-receiving-mail")
        }
        "No attachment was added because one file exceeds 25 MiB." => {
            Some("mail-no-attachment-was-added-because-one-file-exceeds-25-mib")
        }
        "No attachment was added because the 10-file limit would be exceeded." => {
            Some("mail-no-attachment-was-added-because-the-10-file-limit-would-be-exceeded")
        }
        "No attachment was added because the 50 MiB message limit would be exceeded." => {
            Some("mail-no-attachment-was-added-because-the-50-mib-message-limit-would-be-exceeded")
        }
        "No desktop link opener is available on this system." => {
            Some("mail-no-desktop-link-opener-is-available-on-this-system")
        }
        "No favorites yet. Right-click a mailbox in the sidebar to add one." => {
            Some("mail-no-favorites-yet-right-click-a-mailbox-in-the-sidebar-to-add-one")
        }
        "No mail server is set up to ask." => Some("mail-no-mail-server-is-set-up-to-ask"),
        "No message selected" => Some("mail-no-message-selected"),
        "No messages" => Some("mail-no-messages"),
        "No more accounts can be added. Edit an existing account instead." => {
            Some("mail-no-more-accounts-can-be-added-edit-an-existing-account-instead")
        }
        "No rules yet" => Some("mail-no-rules-yet"),
        "No server details attached. Use Add Account to configure a new one." => {
            Some("mail-no-server-details-attached-use-add-account-to-configure-a-new-one")
        }
        "None" => Some("mail-none"),
        "Nothing in this search can be asked of the server." => {
            Some("mail-nothing-in-this-search-can-be-asked-of-the-server")
        }
        "OAuth 2.0" => Some("mail-oauth-2-0"),
        "Only regular local files can be attached." => {
            Some("mail-only-regular-local-files-can-be-attached")
        }
        "Open" => Some("mail-open"),
        "Open Link" => Some("mail-open-link"),
        "Open external link?" => Some("mail-open-external-link"),
        "Outgoing" => Some("mail-outgoing"),
        "Outgoing port" => Some("mail-outgoing-port"),
        "POP3" => Some("mail-pop3"),
        "Password" => Some("mail-password"),
        "Pending send — Undo available" => Some("mail-pending-send-undo-available"),
        "Pin mailboxes you use most to the top of the sidebar" => {
            Some("mail-pin-mailboxes-you-use-most-to-the-top-of-the-sidebar")
        }
        "Plain" => Some("mail-plain"),
        "Plain text" => Some("mail-plain-text"),
        "Ports, encryption and authentication" => Some("mail-ports-encryption-and-authentication"),
        "Preparing message" => Some("mail-preparing-message"),
        "Previous conversation" => Some("mail-previous-conversation"),
        "Protocol" => Some("mail-protocol"),
        "Purple text" => Some("mail-purple-text"),
        "Quick choices" => Some("mail-quick-choices"),
        "Reading" => Some("mail-reading"),
        "Red text" => Some("mail-red-text"),
        "Refresh" => Some("mail-refresh"),
        "Refreshing: fetching the newest messages from the server" => {
            Some("mail-refreshing-fetching-the-newest-messages-from-the-server")
        }
        "Remote content" => Some("mail-remote-content"),
        "Remote images stay blocked until a privacy proxy is available." => {
            Some("mail-remote-images-stay-blocked-until-a-privacy-proxy-is-available")
        }
        "Remove" => Some("mail-remove"),
        "Remove Account" => Some("mail-remove-account"),
        "Remove Snooze" => Some("mail-remove-snooze"),
        "Remove action" => Some("mail-remove-action"),
        "Remove condition" => Some("mail-remove-condition"),
        "Reply" => Some("mail-reply"),
        "Reply All" => Some("mail-reply-all"),
        "Reply all" => Some("mail-reply-all-label"),
        "Reply… (Ctrl+Enter to send)" => Some("mail-reply-ctrl-enter-to-send"),
        "Retry" => Some("mail-retry"),
        "Retry or cancel the current mail operation first." => {
            Some("mail-retry-or-cancel-the-current-mail-operation-first")
        }
        "Retry or cancel the queued message before composing another." => {
            Some("mail-retry-or-cancel-the-queued-message-before-composing-another")
        }
        "Retry saving" => Some("mail-retry-saving"),
        "Rich" => Some("mail-rich"),
        "Rich text" => Some("mail-rich-text"),
        "Rule name" => Some("mail-rule-name"),
        "Rules" => Some("mail-rules"),
        "Rules not applied" => Some("mail-rules-not-applied"),
        "Rules run top-to-bottom over your mailbox." => {
            Some("mail-rules-run-top-to-bottom-over-your-mailbox")
        }
        "SMTP (Outgoing)" => Some("mail-smtp-outgoing"),
        "SSL/TLS" => Some("mail-ssl-tls"),
        "STARTTLS" => Some("mail-starttls"),
        "Safe defaults are in use. Saved rules and sender decisions are protected and will not be overwritten." => {
            Some(
                "mail-safe-defaults-are-in-use-saved-rules-and-sender-decisions-are-protected-and-will-not-be-overwritten",
            )
        }
        "Save" => Some("mail-save"),
        "Save Changes" => Some("mail-save-changes"),
        "Save Draft" => Some("mail-save-draft"),
        "Save attachment" => Some("mail-save-attachment"),
        "Save changes" => Some("mail-save-changes-label"),
        "Save rule" => Some("mail-save-rule"),
        "Saved account identities are inconsistent. Restore valid local state before adding another account." => {
            Some(
                "mail-saved-account-identities-are-inconsistent-restore-valid-local-state-before-adding-another-account",
            )
        }
        "Saving Outbox" => Some("mail-saving-outbox"),
        "Saving did not finish before the close deadline." => {
            Some("mail-saving-did-not-finish-before-the-close-deadline")
        }
        "Saving draft locally…" => Some("mail-saving-draft-locally"),
        "Saving the credential transition before contacting the keyring…" => {
            Some("mail-saving-the-credential-transition-before-contacting-the-keyring")
        }
        "Saving the updated account…" => Some("mail-saving-the-updated-account"),
        "Schedule" => Some("mail-schedule"),
        "Schedule send" => Some("mail-schedule-send"),
        "Schedule without an attachment" => Some("mail-schedule-without-an-attachment"),
        "Scheduled" => Some("mail-scheduled"),
        "Scheduled and Snoozed stay visible while mail is waiting." => {
            Some("mail-scheduled-and-snoozed-stay-visible-while-mail-is-waiting")
        }
        "Search" => Some("mail-search"),
        "Search results" => Some("mail-search-results"),
        "Search scope" => Some("mail-search-scope"),
        "Search the server for more" => Some("mail-search-the-server-for-more"),
        "Searching the server…" => Some("mail-searching-the-server"),
        "Selected text actions" => Some("mail-selected-text-actions"),
        "Send" => Some("mail-send"),
        "Send later" => Some("mail-send-later"),
        "Send message" => Some("mail-send-message"),
        "Send now" => Some("mail-send-now"),
        "Send without attachment" => Some("mail-send-without-attachment"),
        "Sending message" => Some("mail-sending-message"),
        "Sent" => Some("mail-sent"),
        "Server" => Some("mail-server"),
        "Server action not started" => Some("mail-server-action-not-started"),
        "Server change already pending" => Some("mail-server-change-already-pending"),
        "Server change not confirmed" => Some("mail-server-change-not-confirmed"),
        "Server mutation reconciliation pending" => {
            Some("mail-server-mutation-reconciliation-pending")
        }
        "Settings…" => Some("mail-settings"),
        "Settling mail operations and saving data before closing…" => {
            Some("mail-settling-mail-operations-and-saving-data-before-closing")
        }
        "Shortcuts" => Some("mail-shortcuts"),
        "Show HTML" => Some("mail-show-html"),
        "Sidebar" => Some("mail-sidebar"),
        "Sign in again" => Some("mail-sign-in-again"),
        "Sign-in" => Some("mail-sign-in"),
        "Signature" => Some("mail-signature"),
        "Signature will be stored but not auto-inserted." => {
            Some("mail-signature-will-be-stored-but-not-auto-inserted")
        }
        "Smart Mailboxes" => Some("mail-smart-mailboxes"),
        "Snooze" => Some("mail-snooze"),
        "Snooze until" => Some("mail-snooze-until"),
        "Snoozed" => Some("mail-snoozed"),
        "Still loading your saved mail" => Some("mail-still-loading-your-saved-mail"),
        "Subject" => Some("mail-subject"),
        "That action was not applied. Please try again once your saved mail, preferences and shortcuts have finished loading." => {
            Some(
                "mail-that-action-was-not-applied-please-try-again-once-your-saved-mail-preferences-and-shortcuts-have-finished-loading",
            )
        }
        "That location is not a local file — nothing to attach." => {
            Some("mail-that-location-is-not-a-local-file-nothing-to-attach")
        }
        "The SMTP server accepted the message." => {
            Some("mail-the-smtp-server-accepted-the-message")
        }
        "The account changed after removal was requested." => {
            Some("mail-the-account-changed-after-removal-was-requested")
        }
        "The account changed while authentication was settling." => {
            Some("mail-the-account-changed-while-authentication-was-settling")
        }
        "The account changed while publication was settling." => {
            Some("mail-the-account-changed-while-publication-was-settling")
        }
        "The account changed; reopen Edit Account." => {
            Some("mail-the-account-changed-reopen-edit-account")
        }
        "The account password exceeds the supported size." => {
            Some("mail-the-account-password-exceeds-the-supported-size")
        }
        "The account was removed while authentication was settling." => {
            Some("mail-the-account-was-removed-while-authentication-was-settling")
        }
        "The attachment could not be read because access was denied." => {
            Some("mail-the-attachment-could-not-be-read-because-access-was-denied")
        }
        "The attachment could not be read because the file is missing." => {
            Some("mail-the-attachment-could-not-be-read-because-the-file-is-missing")
        }
        "The attachment could not be read." => Some("mail-the-attachment-could-not-be-read"),
        "The attachment could not be saved because access was denied." => {
            Some("mail-the-attachment-could-not-be-saved-because-access-was-denied")
        }
        "The attachment could not be saved because storage is full." => {
            Some("mail-the-attachment-could-not-be-saved-because-storage-is-full")
        }
        "The attachment could not be saved." => Some("mail-the-attachment-could-not-be-saved"),
        "The attachment may have been saved, but durable publication could not be confirmed." => {
            Some(
                "mail-the-attachment-may-have-been-saved-but-durable-publication-could-not-be-confirmed",
            )
        }
        "The attachment may have been saved, but storage durability could not be confirmed." => {
            Some(
                "mail-the-attachment-may-have-been-saved-but-storage-durability-could-not-be-confirmed",
            )
        }
        "The attachment name or media type is invalid." => {
            Some("mail-the-attachment-name-or-media-type-is-invalid")
        }
        "The attachment payload changed and was not used." => {
            Some("mail-the-attachment-payload-changed-and-was-not-used")
        }
        "The attachment payload identity is invalid." => {
            Some("mail-the-attachment-payload-identity-is-invalid")
        }
        "The attachment payload is no longer available locally." => {
            Some("mail-the-attachment-payload-is-no-longer-available-locally")
        }
        "The attachment worker is unavailable." => {
            Some("mail-the-attachment-worker-is-unavailable")
        }
        "The attachment worker stopped unexpectedly." => {
            Some("mail-the-attachment-worker-stopped-unexpectedly")
        }
        "The conversation leaves its mailbox and comes back unread at this time." => {
            Some("mail-the-conversation-leaves-its-mailbox-and-comes-back-unread-at-this-time")
        }
        "The current mailbox has no configured account transport." => {
            Some("mail-the-current-mailbox-has-no-configured-account-transport")
        }
        "The durable Outbox copy is being encoded for submission." => {
            Some("mail-the-durable-outbox-copy-is-being-encoded-for-submission")
        }
        "The durable Outbox copy is being submitted." => {
            Some("mail-the-durable-outbox-copy-is-being-submitted")
        }
        "The encoded message exceeds the 64 MiB sending limit." => {
            Some("mail-the-encoded-message-exceeds-the-64-mib-sending-limit")
        }
        "The exact attachment descriptor could not be handed off, or its stored name changed." => {
            Some(
                "mail-the-exact-attachment-descriptor-could-not-be-handed-off-or-its-stored-name-changed",
            )
        }
        "The link could not be attached to this window." => {
            Some("mail-the-link-could-not-be-attached-to-this-window")
        }
        "The link could not be opened." => Some("mail-the-link-could-not-be-opened"),
        "The mail backend returned an unexpected authentication result." => {
            Some("mail-the-mail-backend-returned-an-unexpected-authentication-result")
        }
        "The mail worker returned an unexpected completion." => {
            Some("mail-the-mail-worker-returned-an-unexpected-completion")
        }
        "The mail-operation counter has no room for another attempt. Restart the app to reissue mail work." => {
            Some(
                "mail-the-mail-operation-counter-has-no-room-for-another-attempt-restart-the-app-to-reissue-mail-work",
            )
        }
        "The message identifier is not safe to send. Restart the message." => {
            Some("mail-the-message-identifier-is-not-safe-to-send-restart-the-message")
        }
        "The message was not encoded for submission." => {
            Some("mail-the-message-was-not-encoded-for-submission")
        }
        "The outgoing sender or recipient list is invalid." => {
            Some("mail-the-outgoing-sender-or-recipient-list-is-invalid")
        }
        "The portal refused permission to open the link." => {
            Some("mail-the-portal-refused-permission-to-open-the-link")
        }
        "The previous credential could not be restored." => {
            Some("mail-the-previous-credential-could-not-be-restored")
        }
        "The private attachment store is unavailable." => {
            Some("mail-the-private-attachment-store-is-unavailable")
        }
        "The selected account has no configured mail transport." => {
            Some("mail-the-selected-account-has-no-configured-mail-transport")
        }
        "The selected save location is not a local file." => {
            Some("mail-the-selected-save-location-is-not-a-local-file")
        }
        "The server accepted the message, but the local Sent copy still needs a valid local timestamp." => {
            Some(
                "mail-the-server-accepted-the-message-but-the-local-sent-copy-still-needs-a-valid-local-timestamp",
            )
        }
        "The system credential service is still starting." => {
            Some("mail-the-system-credential-service-is-still-starting")
        }
        "The updated account and credential are active." => {
            Some("mail-the-updated-account-and-credential-are-active")
        }
        "The updated account configuration is saved, but absence of the exact old credential was not confirmed." => {
            Some(
                "mail-the-updated-account-configuration-is-saved-but-absence-of-the-exact-old-credential-was-not-confirmed",
            )
        }
        "There is no pending message to prepare." => {
            Some("mail-there-is-no-pending-message-to-prepare")
        }
        "This Mailbox" => Some("mail-this-mailbox"),
        "This account was not added because its local identity is already in use." => {
            Some("mail-this-account-was-not-added-because-its-local-identity-is-already-in-use")
        }
        "This build is not permitted to open external links." => {
            Some("mail-this-build-is-not-permitted-to-open-external-links")
        }
        "This desktop's portal does not support opening external links." => {
            Some("mail-this-desktop-s-portal-does-not-support-opening-external-links")
        }
        "This mailbox is empty." => Some("mail-this-mailbox-is-empty"),
        "This weekend" => Some("mail-this-weekend"),
        "Time" => Some("mail-time"),
        "To" => Some("mail-to"),
        "Today" => Some("mail-today"),
        "Tomorrow" => Some("mail-tomorrow"),
        "Trash" => Some("mail-trash"),
        "Triage" => Some("mail-triage"),
        "Try again" => Some("mail-try-again"),
        "Unblock Sender" => Some("mail-unblock-sender"),
        "Undo" => Some("mail-undo"),
        "Unflag" => Some("mail-unflag"),
        "Unknown sender" => Some("mail-unknown-sender"),
        "Unread" => Some("mail-unread"),
        "Unread conversation" => Some("mail-unread-conversation"),
        "Username" => Some("mail-username"),
        "Uses darker colors when Mail is dark. Images keep their original colors." => {
            Some("mail-uses-darker-colors-when-mail-is-dark-images-keep-their-original-colors")
        }
        "Welcome to Mail" => Some("mail-welcome-to-mail"),
        "When a message references remote images or tracking pixels:" => {
            Some("mail-when-a-message-references-remote-images-or-tracking-pixels")
        }
        "When off, the signature is preserved but not auto-inserted." => {
            Some("mail-when-off-the-signature-is-preserved-but-not-auto-inserted")
        }
        "Write your message..." => Some("mail-write-your-message"),
        "Yesterday" => Some("mail-yesterday"),
        "You mentioned an attachment, but no file is attached." => {
            Some("mail-you-mentioned-an-attachment-but-no-file-is-attached")
        }
        "Your name" => Some("mail-your-name"),
        "all" => Some("mail-all-label"),
        "any" => Some("mail-any"),
        "begins with" => Some("mail-begins-with"),
        "contains" => Some("mail-contains"),
        "ends with" => Some("mail-ends-with"),
        "is" => Some("mail-is"),
        "your desktop quits the application with Ctrl+Q." => {
            Some("mail-your-desktop-quits-the-application-with-ctrl-q")
        }
        "Show quoted text" => Some("mail-show-quoted-text"),
        "Hide quoted text" => Some("mail-hide-quoted-text"),
        _ => None,
    }
}
