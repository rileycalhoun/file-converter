
window.addEventListener('DOMContentLoaded', () => {
	let session_id = document.getElementsByTagName("head")[0]
		.getAttribute("session");
	
	if (session_id == null) {
		console.log("session_id not found");
		return;
	}

	console.log(session_id);

	let socket = new WebSocket("http://127.0.0.1:8000/ws");
	socket.addEventListener('open', () => {
		socket.send(`session-id;${session_id}`);
	});

	socket.addEventListener('message', (msg) => {
		console.log(msg);
	});
});
