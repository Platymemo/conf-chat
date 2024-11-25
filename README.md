# conf-chat
Conf-Chat is a terminal-based peer-to-peer messaging app.

Conf-Chat uses Kademlia for in-network storage, and gossipsub for live-chat features.

Users can register with `/register <username> <password>`, and login with `/login <username> <password>`
Additionally, users can change their password with `/update <oldPassword> <newPassword>`

To begin chatting, users can create chatrooms with `/create <roomName>`. Then, they can add other users to the chat with `/add <username>`. To do so however, you must already be friends with that user. This can be achieved with `/friend <username>`. Once both users have friended each other, you will be visible to each other via the `/viewFriends` command. Finally, there is `/msg <username> <message>` to message a friend, whether they are online or not.