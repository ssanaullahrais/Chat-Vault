import { useState } from "react";
import { FileText, ScrollText, ShieldCheck } from "lucide-react";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";

export type LegalDoc = "privacy" | "terms" | "disclaimer";

const UPDATED = "November 25, 2026";

type Section = { title: string; body: string[] };

const DOCS: Record<LegalDoc, { title: string; summary: string; sections: Section[] }> = {
  privacy: {
    title: "Privacy Policy",
    summary: "How ChatVault handles your information.",
    sections: [
      {
        title: "1. Introduction",
        body: [
          "This Privacy Policy explains how the ChatVault desktop application (“ChatVault”, “the application”) handles information. ChatVault is software that runs on your computer. It has no user accounts, no advertising and no usage analytics, and the maintainer operates no server that receives your chats. The only optional online feature is the Google Drive backup described in Section 5, which you can switch on by connecting your own Google account.",
        ],
      },
      {
        title: "2. Information the application accesses",
        body: [
          "To present your conversations, ChatVault reads the local storage locations and databases of the AI tools installed on your computer, such as Claude Code, Codex, Cursor, Gemini CLI, OpenCode and Cline. These conversations may contain any information you or the tools placed in them, including source code, file paths, names, credentials and other confidential or personal data.",
          "ChatVault creates a search index (titles, dates, folders and searchable text) in a database in your local application data folder. This index is stored only on your computer.",
        ],
      },
      {
        title: "3. Files the application creates",
        body: [
          "At your request, or when you enable automatic backup, ChatVault creates copies of conversation files in the “ChatVault Backup” folder in your Documents folder, handoff files in the “ChatVault Handoffs” folder, and Markdown or image files that you export or save. These files are not encrypted and are accessible to anyone who has access to your user account or the relevant folder. You are responsible for their protection.",
        ],
      },
      {
        title: "4. Network communication",
        body: [
          "ChatVault contacts GitHub to check whether a newer version is available and, if so, to download it. In doing so, GitHub receives the technical information inherent in any internet request, such as your IP address, and is governed by its own privacy policy. Web links you select open in your default browser.",
          "If you connect Google Drive, ChatVault also talks to Google (oauth2.googleapis.com and www.googleapis.com) to sign you in and to copy your backup to and from your own Google Drive, as described in Section 5.",
          "ChatVault does not transmit your conversations, backups or index to the maintainer or to any other third party.",
        ],
      },
      {
        title: "5. Google Drive backup (optional)",
        body: [
          "Google Drive backup is off until you choose Connect Google Drive in Sync & backup and approve access on Google's own sign-in page in your browser. ChatVault never sees your Google password. Each person connects their own Google account; nothing is stored in the maintainer's Drive.",
          "ChatVault asks for one permission only, drive.appdata: a private, hidden app folder in your Google Drive. ChatVault can read and write only the files in that folder and cannot see or change any other file in your Drive. With that permission it also reads your account's name and email address, only to show which account is connected.",
          "What is uploaded: the files in your backup folder (Section 3), compressed, plus one small file per PC with that PC's name and the time of its last sync, so your PCs can show each other. Files travel directly between your PC and your Google Drive over HTTPS, with no server in between. Files from your Drive are used only to bring the chats of your other PCs into your backup folder and, when you ask, to restore them into your tools. A sync never deletes anything; when the same chat changed on two PCs, both copies are kept.",
          "The long-lived sign-in (refresh token) is kept only on your PC, in the Windows Credential Manager; the short-lived access token is kept only in memory. Tokens are never written to files, logs or error messages and are sent only to Google.",
          "You can stop at any time: Disconnect in Sync & backup revokes ChatVault's access with Google and removes the sign-in from your PC, and can also delete the backup from your Drive. You can also remove access at myaccount.google.com/connections, or delete the hidden app data in Google Drive under Settings, Manage apps. The backup counts towards your Google storage and is subject to Google's own terms.",
        ],
      },
      {
        title: "6. Google API Services User Data Policy",
        body: [
          "ChatVault's use and transfer to any other app of information received from Google APIs will adhere to the Google API Services User Data Policy, including the Limited Use requirements. Google user data is used only to provide the Google Drive backup and sync you turn on; it is not transferred to anyone except as needed for that feature, for security or to comply with the law; it is not used for advertising, not sold, and not used to determine credit-worthiness; no person reads it; and it is not used to develop, improve or train any generalised artificial intelligence or machine learning model.",
        ],
      },
      {
        title: "7. Third-party tools",
        body: [
          "When you use features such as Continue, Fork or Hand off, ChatVault writes the conversation to a handoff file and launches a third-party tool on your computer to read it. From that point the third-party tool processes the content in accordance with its own terms and privacy policy and may transmit it to its provider. You are responsible for reviewing those terms before sharing any sensitive content.",
        ],
      },
      {
        title: "8. Retention and deletion",
        body: [
          "The maintainer keeps no information about you. You may delete the index, the backup folder, the handoff folder and any exported file at any time by deleting them from your device. Uninstalling the application does not remove the files described in Section 3. Deleting a conversation within ChatVault moves the original file to the Recycle Bin; it is permanently removed only when the Recycle Bin is emptied.",
          "If you use Google Drive backup, the sign-in is removed from your PC when you disconnect, and the backup stays in your Drive until you delete it (Section 5).",
        ],
      },
      {
        title: "9. Children",
        body: ["ChatVault is not directed at children and does not knowingly collect information from any person."],
      },
      {
        title: "10. Changes to this policy",
        body: ["This policy may be updated as the application changes. The date shown above indicates the most recent revision. Continued use of the application after a revision constitutes acceptance of the revised policy. The full policy is also published at ssanaullahrais.github.io/Chat-Vault/privacy.html."],
      },
      {
        title: "11. Contact",
        body: ["Questions about this policy may be raised through the project's issue tracker on GitHub."],
      },
    ],
  },
  terms: {
    title: "Terms & Conditions",
    summary: "Conditions for using ChatVault.",
    sections: [
      {
        title: "1. Acceptance of terms",
        body: ["By installing, accessing or using ChatVault you agree to be bound by these Terms & Conditions. If you do not agree, you must not install or use the application. If you use ChatVault on behalf of an organization, you represent that you are authorized to accept these terms on its behalf."],
      },
      {
        title: "2. Description of the application",
        body: [
          "ChatVault is a free, open source desktop application for viewing, searching, exporting, backing up and continuing conversations that AI tools have stored on your computer. ChatVault is independent. It is not affiliated with, endorsed by or sponsored by Anthropic, OpenAI, Google, Anysphere (Cursor), Cline, OpenCode or any other provider. All product names, logos and trademarks belong to their respective owners and are used only to identify the tool from which a conversation originates.",
        ],
      },
      {
        title: "3. Responsibility for data",
        body: [
          "You are solely responsible for all data on your computer and all data processed through ChatVault, including the content of your conversations, the maintenance of your own backups, and the protection of any file the application creates, which is not encrypted.",
          "You are also responsible for complying with all applicable laws, regulations, contractual obligations and employer or client policies, and with the terms of every AI tool you use, including those concerning personal data, confidential information, credentials, intellectual property and cross-border data transfer.",
        ],
      },
      {
        title: "4. Operations that modify files",
        body: [
          "Certain features create, copy, restore, rename, archive or delete files, or launch other programs on your computer. Restoring may add conversation files back into the storage folders of other tools. Deleting moves a conversation file to the Recycle Bin. You should read the description of any action before you use it. The maintainer and contributors are not responsible for any file that is lost, overwritten, damaged or exposed, regardless of cause.",
        ],
      },
      {
        title: "5. Third-party tools and services",
        body: [
          "ChatVault may launch or interoperate with third-party tools and open third-party websites. These are outside the control of the maintainer. The maintainer is not responsible for their operation, for the data they transmit to their providers, for how they store information, or for any charges they incur.",
          "Google Drive backup is optional. If you connect it, your use of Google Drive is governed by Google's terms, the backup uses your own Google storage, and you must not use it to store content that breaks Google's terms or policies. ChatVault's use of Google data follows the Google API Services User Data Policy, as described in the Privacy Policy. You can disconnect at any time.",
        ],
      },
      {
        title: "6. Disclaimer of warranties",
        body: [
          "ChatVault is provided “as is” and “as available”, without warranty of any kind, whether express or implied, including any warranty of merchantability, fitness for a particular purpose, accuracy, completeness, availability, security or non-infringement. Third-party tools may change the way they store conversations at any time, and as a result some conversations may be missing, incomplete or unreadable.",
        ],
      },
      {
        title: "7. Limitation of liability",
        body: [
          "To the maximum extent permitted by applicable law, the maintainer and contributors shall not be liable for any direct, indirect, incidental, special, consequential, exemplary or punitive damages, or for any loss of data, profits, goodwill or business, or disclosure of confidential information, arising out of or in connection with the use of, or inability to use, ChatVault, even if advised of the possibility of such damages. Where such exclusion is not permitted, liability is limited to the minimum extent the law allows. You use ChatVault at your own risk.",
        ],
      },
      {
        title: "8. Acceptable use",
        body: ["You may not use ChatVault to access conversations, accounts or computers that you are not authorized to access, to violate any law or the rights of others, or to cause harm. You are responsible for the manner in which you use the application."],
      },
      {
        title: "9. Open source licence and updates",
        body: ["The source code of ChatVault is published on GitHub, and the licence stated in its repository governs your use and distribution of the code. The application may update itself, and features may be changed or withdrawn at any time without notice."],
      },
      {
        title: "10. Changes to these terms",
        body: ["These terms may be revised from time to time. Continued use of the application after a revision constitutes acceptance of the revised terms."],
      },
      {
        title: "11. Governing law and contact",
        body: ["These terms are governed by the laws applicable in the jurisdiction in which the maintainer resides, without regard to conflict-of-law principles, except where mandatory local law provides otherwise. Questions may be raised through the project's issue tracker on GitHub."],
      },
    ],
  },
  disclaimer: {
    title: "Disclaimer",
    summary: "Your responsibility for your data.",
    sections: [
      {
        title: "1. Use at your own risk",
        body: [
          "ChatVault is provided free of charge and without warranty of any kind. The maintainer and contributors do not guarantee that the software will operate without interruption or error, that it will find, display or preserve every conversation, or that any file it reads, copies, exports, restores or deletes will remain intact.",
        ],
      },
      {
        title: "2. Responsibility for data",
        body: [
          "You are solely and entirely responsible for all data processed through ChatVault. This includes the content of every conversation and file the application displays, copies, exports or backs up; who has access to that content; where it is stored; whether you are permitted to keep, copy or share it; and whether you maintain separate copies.",
        ],
      },
      {
        title: "3. No liability for loss or misuse",
        body: [
          "The maintainer and contributors accept no responsibility or liability for data that is lost, altered, deleted, corrupted, disclosed or misused, whether through the use of ChatVault, through a third-party tool, or otherwise. This includes anything a third-party AI tool does with a conversation you hand to it, and any decision you make on the basis of information the application displays.",
        ],
      },
      {
        title: "4. Your obligations",
        body: [
          "Maintain your own independent backups of anything important. Do not store secrets, credentials or regulated data in conversations that you cannot adequately protect. Comply with the laws, contracts and policies that apply to you and to the data you handle.",
        ],
      },
      {
        title: "5. No professional advice",
        body: [
          "Nothing in ChatVault or in these documents constitutes legal, financial, security or other professional advice. If you use ChatVault for work or for the data of other people, consult a qualified professional to determine whether further safeguards or agreements are required.",
        ],
      },
    ],
  },
};

const TABS: { id: LegalDoc; label: string; icon: typeof FileText }[] = [
  { id: "privacy", label: "Privacy", icon: ShieldCheck },
  { id: "terms", label: "Terms", icon: ScrollText },
  { id: "disclaimer", label: "Disclaimer", icon: FileText },
];

/** Privacy Policy, Terms & Conditions and the plain-words disclaimer, in one window with tabs. */
export function LegalDialog({ doc, onDoc, onClose }: { doc: LegalDoc | null; onDoc: (d: LegalDoc) => void; onClose: () => void }) {
  const [seen, setSeen] = useState<LegalDoc>("privacy");
  const current = doc ?? seen;
  const d = DOCS[current];
  return (
    <Dialog open={!!doc} onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="flex max-h-[90dvh] flex-col gap-3 sm:max-w-2xl">
        <DialogHeader className="gap-0.5">
          <DialogTitle className="text-base font-medium">{d.title}</DialogTitle>
          <DialogDescription>
            {d.summary} Last updated {UPDATED}.
          </DialogDescription>
        </DialogHeader>
        <Tabs
          value={current}
          onValueChange={(v) => {
            setSeen(v as LegalDoc);
            onDoc(v as LegalDoc);
          }}
        >
          <TabsList>
            {TABS.map((t) => (
              <TabsTrigger key={t.id} value={t.id} className="gap-1.5">
                <t.icon className="size-3.5" /> {t.label}
              </TabsTrigger>
            ))}
          </TabsList>
        </Tabs>
        <ScrollArea className="max-h-[56dvh] min-h-0 rounded-lg border" viewportClassName="max-h-[inherit]">
          <div className="space-y-5 p-4 pr-5 text-sm leading-relaxed">
            {d.sections.map((s) => (
              <section key={s.title}>
                <h3 className="mb-1.5 font-medium">{s.title}</h3>
                <div className="space-y-2 text-muted-foreground">
                  {s.body.map((para) => (
                    <p key={para}>{para}</p>
                  ))}
                </div>
              </section>
            ))}
          </div>
        </ScrollArea>
      </DialogContent>
    </Dialog>
  );
}
