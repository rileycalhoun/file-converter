
window.onload = async () => {
	let form = document.getElementById('convert');
	form.addEventListener('submit', async function(event) {
		event.preventDefault();
		let response = await fetch(event.target.action, {
			method: 'POST',
			body: new FormData(form)
		});	

		let message = await response.text();
		alert(`${response.status}, ${message}`);
	});
}
